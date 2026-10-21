//! The page in each delivery mode.

use super::hub::Snapshot;
use super::metrics::ConnGuard;
use super::{Hub, Metrics, Mode};
use crate::render::{
    metrics_tag, render_page, script_tag, Grain, PageInfo, PatchSet, LIVE_JS, METRICS_JS, PICKER,
};
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
    Router::new()
        .route("/", get(page))
        .route("/live.js", get(live_js))
        .route("/live", get(live_ws))
        .route("/metrics.js", get(metrics_js))
        .route("/metrics", get(metrics_get))
        .route("/metrics/client", axum::routing::post(metrics_post))
        .with_state(App {
            hub,
            metrics,
            options,
        })
}

async fn page(State(app): State<App>, Query(p): Query<Params>, headers: HeaderMap) -> Response {
    match p.live.as_deref() {
        None => refresh(&app, &p, &headers, true),
        Some("refresh") => refresh(&app, &p, &headers, false),
        Some("stream") => match grain(&p, Grain::Region) {
            Ok(g) => stream(&app, g, p.polyfill == Some(1), p.metrics == Some(1)),
            Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
        },
        Some("ws") => match grain(&p, Grain::Node) {
            Ok(g) => ws_page(&app, g, p.metrics == Some(1)),
            Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
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
/// With `auto`, the page with no `live` parameter: the refresh only inside
/// `<noscript>`, and a script that picks stream or WebSocket.
fn refresh(app: &App, p: &Params, headers: &HeaderMap, auto: bool) -> Response {
    let now = app.hub.current();
    // The header shows the connection state, so it's part of the page's identity.
    let state = if now.connected { "live" } else { "off" };
    let etag = format!("\"{}-{state}\"", now.snapshot);
    let unchanged = !now.snapshot.is_empty()
        && headers
            .get(header::IF_NONE_MATCH)
            .is_some_and(|v| v.as_bytes() == etag.as_bytes());
    if unchanged {
        app.metrics.refresh_status(304);
        return StatusCode::NOT_MODIFIED.into_response();
    }
    app.metrics.refresh_status(200);
    let meta = format!(
        "<meta http-equiv=\"refresh\" content=\"{}\">",
        p.every.unwrap_or(1)
    );
    let head = if auto {
        format!("<noscript>{meta}</noscript>")
    } else {
        meta
    };
    let mut body_end = metrics_tag(p.metrics == Some(1));
    if auto {
        body_end.push_str(PICKER);
    }
    let mut resp = html(render_page(
        &now.region_html,
        &PageInfo {
            doc: app.hub.doc(),
            snapshot: &now.snapshot,
            connected: now.connected,
            seq: now.seq,
            sent_ms: now.sent_ms,
            head: &head,
            body_end: &body_end,
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
fn grain(p: &Params, default: Grain) -> Result<Grain, &'static str> {
    match p.grain.as_deref() {
        None => Ok(default),
        Some("node") => Ok(Grain::Node),
        Some("region") => Ok(Grain::Region),
        Some(_) => Err("grain must be node or region\n"),
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

/// One patch set as a viewer receives it.
pub(crate) struct Sent {
    pub seq: u64,
    pub sent_ms: u64,
    pub html: String,
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

    /// The next patch set, or `None` once the hub is gone.
    pub(crate) async fn next(&mut self) -> Option<Sent> {
        loop {
            match self.rx.recv().await {
                Ok(p) if p.seq <= self.after => continue,
                Ok(p) => {
                    self.after = p.seq;
                    return Some(Sent {
                        seq: p.seq,
                        sent_ms: p.sent_ms,
                        html: p.html(self.grain).to_string(),
                    });
                }
                Err(RecvError::Lagged(_)) => {
                    self.metrics.resync();
                    let full = self.hub.full();
                    self.after = full.seq;
                    return Some(Sent {
                        seq: full.seq,
                        sent_ms: full.sent_ms,
                        html: full.html(Grain::Region).to_string(),
                    });
                }
                Err(RecvError::Closed) => return None,
            }
        }
    }
}

/// The patch stream: the page, then the response stays open and each change
/// arrives as `<template for>` children of `<body>`.
fn stream(app: &App, grain: Grain, polyfill: bool, measure: bool) -> Response {
    let guard = app.metrics.opened(Mode::Stream);
    let (feed, now) = Feed::new(&app.hub, &app.metrics, grain);
    let mut script = metrics_tag(measure);
    if polyfill {
        script.push_str(&script_tag("stream", grain));
    }
    let first = open_page(app, &now, "", &script);
    app.metrics.sent(Mode::Stream, first.len(), false);
    let heartbeat = app.options.heartbeat;
    let beat = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    struct St {
        measure: bool,
        feed: Feed,
        beat: tokio::time::Interval,
        metrics: Metrics,
        _guard: ConnGuard,
    }
    let st = St {
        measure,
        feed,
        beat,
        metrics: app.metrics.clone(),
        _guard: guard,
    };
    let rest = futures_util::stream::unfold(st, |mut st| async move {
        let (chunk, patch) = tokio::select! {
            next = st.feed.next() => {
                let next = next?;
                let mut chunk = next.html;
                if st.measure {
                    // Native patches consume their template, data-seq and all, so
                    // metrics.js watches for this instead.
                    chunk.push_str(&format!(
                        "<i hidden data-xtxp-seq=\"{}\" data-xtxp-sent=\"{}\"></i>",
                        next.seq, next.sent_ms
                    ));
                }
                (chunk, true)
            }
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
    use xmesh::{Op, Xmesh};
    use xtxp_muppet::ports::Screen;

    #[tokio::test]
    async fn lagging_viewer_gets_a_region_resync() {
        let hub = Hub::new("logo");
        let metrics = Metrics::new();
        let d = Xmesh::new(7).unwrap();
        d.apply(&Xmesh::seed(include_str!("../../../xmesh/tests/fixtures/xtxp-logo.svg")).unwrap())
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
        let first = feed.next().await.unwrap().html;
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
        let next = feed.next().await.unwrap().html;
        assert!(next.contains("stroke-width=\"1\""), "{next}");
    }
}

async fn live_js() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        LIVE_JS,
    )
        .into_response()
}

/// The WebSocket mode's page: complete, with the polyfill.
fn ws_page(app: &App, grain: Grain, measure: bool) -> Response {
    let now = app.hub.current();
    html(render_page(
        &now.region_html,
        &PageInfo {
            doc: app.hub.doc(),
            snapshot: &now.snapshot,
            connected: now.connected,
            seq: now.seq,
            sent_ms: now.sent_ms,
            head: "",
            body_end: &(metrics_tag(measure) + &script_tag("ws", grain)),
        },
    ))
}

#[derive(Deserialize)]
struct LiveParams {
    grain: Option<String>,
}

async fn live_ws(
    State(app): State<App>,
    Query(p): Query<LiveParams>,
    upgrade: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    let g = match grain(
        &Params {
            grain: p.grain,
            ..Params::default()
        },
        Grain::Node,
    ) {
        Ok(g) => g,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    upgrade.on_upgrade(move |socket| live_socket(app, g, socket))
}

/// One viewer's WebSocket: the whole document first, then each patch set.
async fn live_socket(app: App, grain: Grain, mut socket: axum::extract::ws::WebSocket) {
    use axum::extract::ws::Message;
    let _guard = app.metrics.opened(Mode::Ws);
    let (mut feed, now) = Feed::new(&app.hub, &app.metrics, grain);
    let first = crate::render::template("doc", now.seq, now.sent_ms, &now.region_html);
    app.metrics.sent(Mode::Ws, first.len(), true);
    if socket.send(Message::Text(first.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            next = feed.next() => {
                let Some(Sent { html, .. }) = next else { return };
                app.metrics.sent(Mode::Ws, html.len(), true);
                if socket.send(Message::Text(html.into())).await.is_err() {
                    return;
                }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

async fn metrics_js() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        METRICS_JS,
    )
        .into_response()
}

async fn metrics_get(State(app): State<App>) -> Response {
    let body = app.metrics.to_json(&app.hub.render_us()).to_string();
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

async fn metrics_post(State(app): State<App>, body: String) -> Response {
    match serde_json::from_str::<super::metrics::ClientBatch>(&body) {
        Ok(batch) => {
            app.metrics.client_batch(batch);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e}\n")).into_response(),
    }
}
