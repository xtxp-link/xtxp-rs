//! `serve`: a server-side muppet that watches one document on y-sweet and
//! shows it to browsers, by refresh, by an open response, or over a WebSocket.

pub mod hub;
pub mod metrics;
pub mod modes;

pub use hub::{Hub, HubView, Snapshot};
pub use metrics::{Metrics, MetricsSnapshot, Mode};
pub use modes::{router, Params};

use crate::adapters::ws;
use crate::core::Mode as CoreMode;
use std::net::SocketAddr;

/// Watches `doc` on y-sweet at `server` and serves it on `listen` until Ctrl-C.
pub fn run(server: &str, doc: &str, listen: SocketAddr) -> Result<(), String> {
    let hub = Hub::new(doc);
    let metrics = Metrics::new();
    let (server, doc, view) = (server.to_string(), doc.to_string(), HubView(hub.clone()));
    // The y-sweet side: the watch-only core on its own thread and runtime. It
    // never finishes on its own; it ends with the process.
    std::thread::spawn(move || {
        ws::run(CoreMode::Interactive, &server, &doc, view, None);
    });
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
