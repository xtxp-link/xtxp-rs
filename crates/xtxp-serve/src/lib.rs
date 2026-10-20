//! A muppet app: serves a y-sweet document to browsers in three delivery modes
//! (refresh, stream, WebSocket). The app is the owner of the server-side views.

mod render;
mod serve;

use serve::HubView;
pub use serve::{router, router_with, Hub, Metrics, Options};

use std::net::SocketAddr;

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

/// Starts watching `doc` on y-sweet at `server`: the watch-only core on its
/// own thread and runtime, feeding a new hub. It never finishes on its own;
/// it ends with the process.
pub fn start(server: &str, doc: &str) -> (Hub, Metrics) {
    let hub = Hub::new(doc);
    let (server, doc, view) = (server.to_string(), doc.to_string(), HubView(hub.clone()));
    std::thread::spawn(move || {
        xtxp_muppet::adapters::ws::run(
            xtxp_muppet::core::Mode::Interactive,
            &server,
            &doc,
            view,
            None,
        );
    });
    (hub, Metrics::new())
}
