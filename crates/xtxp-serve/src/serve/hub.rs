//! One document's current page and its patch sets, fanned out to every viewer.

use crate::render::{Page, PatchSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use xtxp_muppet::ports::{Outcome, Screen, View};

pub use crate::render::Snapshot;

/// Patch sets a viewer may fall behind by before it gets a whole-document resync.
pub const CAPACITY: usize = 64;

struct State {
    page: Page,
    render_us: Vec<u64>,
}

/// Render times kept for `/metrics`.
const RENDER_SAMPLES: usize = 10_000;

struct Inner {
    doc: String,
    state: Mutex<State>,
    tx: broadcast::Sender<Arc<PatchSet>>,
}

/// One document's page and its broadcast channel. Cheap to clone.
#[derive(Clone)]
pub struct Hub(Arc<Inner>);

/// The server's clock for `data-sent`: Unix time in milliseconds.
pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Hub {
    pub fn new(doc: &str) -> Hub {
        let (tx, _) = broadcast::channel(CAPACITY);
        Hub(Arc::new(Inner {
            doc: doc.to_string(),
            state: Mutex::new(State {
                page: Page::new(),
                render_us: Vec::new(),
            }),
            tx,
        }))
    }

    pub fn doc(&self) -> &str {
        &self.0.doc
    }

    /// Renders a new screen and publishes its patch set, if anything changed.
    pub fn show(&self, screen: &Screen) {
        let mut state = self.0.state.lock().expect("hub lock");
        let started = std::time::Instant::now();
        let patch = state.page.show(
            &screen.tree,
            &screen.snapshot_hash,
            screen.connected,
            unix_ms(),
        );
        if let Some(patch) = patch {
            let us = started.elapsed().as_micros() as u64;
            if state.render_us.len() == RENDER_SAMPLES {
                state.render_us.remove(0);
            }
            state.render_us.push(us);
            // No receivers is fine: nobody is watching yet.
            let _ = self.0.tx.send(Arc::new(patch));
        }
    }

    /// The current page state, without subscribing.
    pub fn current(&self) -> Snapshot {
        let state = self.0.state.lock().expect("hub lock");
        state.page.snapshot()
    }

    /// Subscribes, then copies the current state under the same lock, so the
    /// viewer misses no patch set newer than what it starts from.
    pub fn subscribe(&self) -> (broadcast::Receiver<Arc<PatchSet>>, Snapshot) {
        let state = self.0.state.lock().expect("hub lock");
        (self.0.tx.subscribe(), state.page.snapshot())
    }

    /// How long each change took to render, in microseconds (both grains).
    pub fn render_us(&self) -> Vec<u64> {
        self.0.state.lock().expect("hub lock").render_us.clone()
    }

    /// The whole document as one region patch.
    pub fn full(&self) -> Arc<PatchSet> {
        let state = self.0.state.lock().expect("hub lock");
        Arc::new(state.page.full(unix_ms()))
    }
}

/// Feeds the hub from the muppet core.
pub struct HubView(pub Hub);

impl View for HubView {
    fn show(&mut self, screen: &Screen) {
        self.0.show(screen)
    }
    fn status(&mut self, line: &str) {
        eprintln!("{line}");
    }
    fn finished(&mut self, _: &Outcome) {}
}
