//! `serve`: serves a y-sweet document to browsers in three delivery modes
//! (refresh, stream, WebSocket). The server-side app that owns the browser view.

pub mod hub;
pub mod metrics;
pub mod modes;

pub use hub::{Hub, HubView};
pub use metrics::{Metrics, Mode};
pub use modes::{router, router_with, Options};
