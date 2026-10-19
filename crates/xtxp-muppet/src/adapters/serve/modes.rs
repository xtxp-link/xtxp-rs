//! The page in each delivery mode.

use super::{Hub, Metrics};
use crate::render::{render_page, PageInfo};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

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
}

pub fn router(hub: Hub, metrics: Metrics) -> Router {
    Router::new()
        .route("/", get(page))
        .with_state(App { hub, metrics })
}

async fn page(State(app): State<App>, Query(p): Query<Params>, headers: HeaderMap) -> Response {
    match p.live.as_deref() {
        None | Some("refresh") => refresh(&app, &p, &headers),
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
