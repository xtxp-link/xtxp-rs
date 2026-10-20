//! `serve`: serves a y-sweet document to browsers in three delivery modes
//! (refresh, stream, WebSocket). The server-side app that owns the browser view.

pub mod hub;
pub mod metrics;
pub mod modes;

pub use hub::{Hub, HubView, Snapshot};
pub use metrics::{Metrics, MetricsSnapshot, Mode, ModeStats};
pub use modes::{router, router_with, Options, Params};

use xtxp_muppet::adapters::ws;
use xtxp_muppet::core::Mode as CoreMode;
use std::net::SocketAddr;

/// Starts watching `doc` on y-sweet at `server`: the watch-only core on its
/// own thread and runtime, feeding a new hub. It never finishes on its own;
/// it ends with the process.
pub fn start(server: &str, doc: &str) -> (Hub, Metrics) {
    let hub = Hub::new(doc);
    let (server, doc, view) = (server.to_string(), doc.to_string(), HubView(hub.clone()));
    std::thread::spawn(move || {
        ws::run(CoreMode::Interactive, &server, &doc, view, None);
    });
    (hub, Metrics::new())
}

/// Watches `doc` on y-sweet at `server` and serves it on `listen` until Ctrl-C.
pub fn run(server: &str, doc: &str, listen: SocketAddr) -> Result<(), String> {
    let (hub, metrics) = start(server, doc);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(listen)
            .await
            .map_err(|e| format!("can't listen on {listen}: {e}"))?;
        eprintln!("serving {} on http://{listen}/", hub.doc());
        axum::serve(listener, router(hub, metrics))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .map_err(|e| e.to_string())
    })
}
