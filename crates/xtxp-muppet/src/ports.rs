//! The muppet's ports. Driving: [`crate::core::Muppet`]'s methods, which the
//! adapters call. Driven: these traits, which the adapters implement.

use xmesh::{IdNode, NodeId};

/// A connection to the host. Non-blocking: `connect` asks for a connection,
/// and the adapter later calls `Muppet::on_connected` or `on_disconnected`.
pub trait Transport {
    fn connect(&mut self);
    fn send(&mut self, frame: Vec<u8>);
    fn close(&mut self);
}

pub trait Clock {
    /// Milliseconds on a monotonic clock.
    fn now_ms(&self) -> u64;
}

pub trait IdSource {
    /// A fresh client ID in `1..=2^32-1` (Yjs and y-sweet read client IDs as 32 bits).
    fn next_client_id(&mut self) -> u64;
}

pub trait View {
    fn show(&mut self, screen: &Screen);
    fn status(&mut self, line: &str);
    fn finished(&mut self, outcome: &Outcome);
}

/// What a view shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Screen {
    pub tree: Vec<IdNode>,
    pub snapshot_hash: String,
    pub doc_hash: String,
    pub connected: bool,
    /// Nodes whose content changed since the previous screen.
    pub changed: Vec<NodeId>,
}

/// How a one-shot mode ended.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    pub ok: bool,
    pub doc_hash: String,
    pub snapshot_hash: String,
    pub updates_before_synced: u32,
    pub canonical: String,
    pub message: Option<String>,
}
