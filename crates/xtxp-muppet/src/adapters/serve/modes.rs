//! The page in each delivery mode.

use super::hub::Snapshot;
use super::metrics::ConnGuard;
use super::{Hub, Metrics, Mode};
use crate::render::{render_page, Grain, PageInfo, PatchSet};
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::broadcast::{error::RecvError, Receiver};

/// The page's query parameters.
#[derive(Debug, Default, Deserialize)]
pub struct Params {
    pub live: Option<String>,
    pub grain: Option<String>,
    pub every: Option<u32>,
    pub metrics: Option<u8>,
    pub polyfill: Option<u8>,
}

#[derive(Clone)]
struct App {
    hub: Hub,
    metrics: Metrics,
    options: Options,
}

/// Server settings.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// How often an open stream gets a heartbeat comment.
    pub heartbeat: std::time::Duration,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            heartbeat: std::time::Duration::from_secs(15),
        }
    }
}

pub fn router(hub: Hub, metrics: Metrics) -> Router {
    router_with(hub, metrics, Options::default())
}

pub fn router_with(hub: Hub, metrics: Metrics, options: Options) -> Router {
    Router::new().route("/", get(page)).with_state(App {
        hub,
        metrics,
        options,
    })
}

async fn page(State(app): State<App>, Query(p): Query<Params>, headers: HeaderMap) -> Response {
    match p.live.as_deref() {
        None | Some("refresh") => refresh(&app, &p, &headers),
        Some("stream") => match grain(&p, Grain::Region) {
            Ok(g) => stream(&app, g),
            Err(r) => r,
        },
        Some(_) => (
            StatusCode::BAD_REQUEST,
            "live must be refresh, stream or ws\n",
        )
            .into_response(),
    }
}

fn html(body: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

/// The baseline: a full page that refreshes itself, and `304` while nothing changed.
fn refresh(app: &App, p: &Params, headers: &HeaderMap) -> Response {
    let now = app.hub.current();
    let etag = format!("\"{}\"", now.snapshot);
    let unchanged = !now.snapshot.is_empty()
        && headers
            .get(header::IF_NONE_MATCH)
            .is_some_and(|v| v.as_bytes() == etag.as_bytes());
    if unchanged {
        app.metrics.refresh_status(304);
        return StatusCode::NOT_MODIFIED.into_response();
    }
    app.metrics.refresh_status(200);
    let head = format!(
        "<meta http-equiv=\"refresh\" content=\"{}\">",
        p.every.unwrap_or(1)
    );
    let mut resp = html(render_page(
        &now.region_html,
        &PageInfo {
            doc: app.hub.doc(),
            snapshot: &now.snapshot,
            connected: now.connected,
            seq: now.seq,
            sent_ms: now.sent_ms,
            head: &head,
            body_end: "",
        },
    ));
    if !now.snapshot.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&etag) {
            resp.headers_mut().insert(header::ETAG, v);
        }
    }
    resp
}

/// The `grain` parameter, or `default`.
fn grain(p: &Params, default: Grain) -> Result<Grain, Response> {
    match p.grain.as_deref() {
        None => Ok(default),
        Some("node") => Ok(Grain::Node),
        Some("region") => Ok(Grain::Region),
        Some(_) => Err((StatusCode::BAD_REQUEST, "grain must be node or region\n").into_response()),
    }
}

/// A page's HTML up to, but not including, `</body></html>`, for a response
/// that stays open.
fn open_page(app: &App, now: &Snapshot, head: &str, body_end: &str) -> String {
    let page = render_page(
        &now.region_html,
        &PageInfo {
            doc: app.hub.doc(),
            snapshot: &now.snapshot,
            connected: now.connected,
            seq: now.seq,
            sent_ms: now.sent_ms,
            head,
            body_end,
        },
    );
    page.trim_end()
        .strip_suffix("</body></html>")
        .unwrap_or(&page)
        .to_string()
}

/// What a viewer receives after its first page: newer patch sets, a region
/// resync when it falls behind, and nothing older than where it started.
pub(crate) struct Feed {
    rx: Receiver<Arc<PatchSet>>,
    after: u64,
    grain: Grain,
    hub: Hub,
    metrics: Metrics,
}

impl Feed {
    pub(crate) fn new(hub: &Hub, metrics: &Metrics, grain: Grain) -> (Feed, Snapshot) {
        let (rx, now) = hub.subscribe();
        let feed = Feed {
            rx,
            after: now.seq,
            grain,
            hub: hub.clone(),
            metrics: metrics.clone(),
        };
        (feed, now)
    }

    /// The next patch set's HTML, or `None` once the hub is gone.
    pub(crate) async fn next(&mut self) -> Option<String> {
        loop {
            match self.rx.recv().await {
                Ok(p) if p.seq <= self.after => continue,
                Ok(p) => {
                    self.after = p.seq;
                    return Some(p.html(self.grain).to_string());
                }
                Err(RecvError::Lagged(_)) => {
                    self.metrics.resync();
                    let full = self.hub.full();
                    self.after = full.seq;
                    return Some(full.html(Grain::Region).to_string());
                }
                Err(RecvError::Closed) => return None,
            }
        }
    }
}

/// The patch stream: the page, then the response stays open and each change
/// arrives as `<template for>` children of `<body>`.
fn stream(app: &App, grain: Grain) -> Response {
    let guard = app.metrics.opened(Mode::Stream);
    let (feed, now) = Feed::new(&app.hub, &app.metrics, grain);
    let first = open_page(app, &now, "", "");
    app.metrics.sent(Mode::Stream, first.len(), false);
    let heartbeat = app.options.heartbeat;
    let beat = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    struct St {
        feed: Feed,
        beat: tokio::time::Interval,
        metrics: Metrics,
        _guard: ConnGuard,
    }
    let st = St {
        feed,
        beat,
        metrics: app.metrics.clone(),
        _guard: guard,
    };
    let rest = futures_util::stream::unfold(st, |mut st| async move {
        let (chunk, patch) = tokio::select! {
            next = st.feed.next() => (next?, true),
            _ = st.beat.tick() => ("<!--hb-->".to_string(), false),
        };
        st.metrics.sent(Mode::Stream, chunk.len(), patch);
        Some((Ok::<_, Infallible>(chunk), st))
    });
    let body = futures_util::stream::once(async move { Ok::<_, Infallible>(first) });
    let body = futures_util::StreamExt::chain(body, rest);
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        Body::from_stream(body),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::Screen;
    use xmesh::{Op, Xmesh};

    #[tokio::test]
    async fn lagging_viewer_gets_a_region_resync() {
        let hub = Hub::new("logo");
        let metrics = Metrics::new();
        let d = Xmesh::new(7).unwrap();
        d.apply(
            &Xmesh::seed(include_str!(
                "../../../../xmesh/tests/fixtures/xtxp-logo.svg"
            ))
            .unwrap(),
        )
        .unwrap();
        let show = |d: &Xmesh| {
            hub.show(&Screen {
                tree: d.tree_with_ids(),
                snapshot_hash: d.snapshot_hash(),
                connected: true,
                ..Screen::default()
            })
        };
        show(&d);
        let (mut feed, _) = Feed::new(&hub, &metrics, Grain::Node);
        for i in 0..70 {
            d.edit(&[Op::SetAttr {
                path: vec![2, 0],
                name: "stroke-width".into(),
                value: format!("{}", 20 + i),
            }])
            .unwrap();
            show(&d);
        }
        let first = feed.next().await.unwrap();
        assert!(first.starts_with("<template for=\"doc\""), "{first}");
        assert!(first.contains("stroke-width=\"89\""));
        assert_eq!(metrics.snapshot().resyncs, 1);
        // Nothing older than the resync follows it.
        d.edit(&[Op::SetAttr {
            path: vec![2, 0],
            name: "stroke-width".into(),
            value: "1".into(),
        }])
        .unwrap();
        show(&d);
        let next = feed.next().await.unwrap();
        assert!(next.contains("stroke-width=\"1\""), "{next}");
    }
}
