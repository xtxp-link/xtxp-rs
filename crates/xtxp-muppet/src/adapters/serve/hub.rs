//! One document's current page and its patch sets, fanned out to every viewer.

use crate::ports::{Outcome, Screen, View};
use crate::render::{state_html, template, PatchSet, Renderer};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

/// Patch sets a viewer may fall behind by before it gets a whole-document resync.
pub const CAPACITY: usize = 64;

const EMPTY_REGION: &str = "<?start name=\"doc\"><?end>";

/// What a viewer starts from.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub region_html: String,
    pub snapshot: String,
    pub connected: bool,
    pub seq: u64,
    pub sent_ms: u64,
}

struct State {
    renderer: Renderer,
    connected: bool,
    sent_ms: u64,
    render_us: Vec<u64>,
}

/// Render times kept for `/metrics`.
const RENDER_SAMPLES: usize = 10_000;

struct Inner {
    doc: String,
    state: Mutex<State>,
    tx: broadcast::Sender<Arc<PatchSet>>,
}

/// One document's renderer and its broadcast channel. Cheap to clone.
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
                renderer: Renderer::new(),
                connected: false,
                sent_ms: 0,
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
        let state_changed = state.connected != screen.connected;
        state.connected = screen.connected;
        let now = unix_ms();
        let started = std::time::Instant::now();
        let patch = state
            .renderer
            .update(&screen.tree, &screen.snapshot_hash, now);
        if patch.is_some() {
            let us = started.elapsed().as_micros() as u64;
            if state.render_us.len() == RENDER_SAMPLES {
                state.render_us.remove(0);
            }
            state.render_us.push(us);
        }
        // A change of connection state patches the header's state marker,
        // with the content's patch set or on its own.
        let patch = match (patch, state_changed) {
            (Some(mut p), true) => {
                let st = template("state", p.seq, now, &state_html(screen.connected));
                p.node_html.push_str(&st);
                p.region_html.push_str(&st);
                Some(p)
            }
            (None, true) => {
                let seq = state.renderer.next_seq();
                let st = template("state", seq, now, &state_html(screen.connected));
                Some(PatchSet {
                    seq,
                    sent_ms: now,
                    snapshot: screen.snapshot_hash.clone(),
                    node_html: st.clone(),
                    region_html: st,
                })
            }
            (p, false) => p,
        };
        if let Some(patch) = patch {
            state.sent_ms = now;
            // No receivers is fine: nobody is watching yet.
            let _ = self.0.tx.send(Arc::new(patch));
        }
    }

    /// The current page state, without subscribing.
    pub fn current(&self) -> Snapshot {
        let state = self.0.state.lock().expect("hub lock");
        snapshot_of(&state)
    }

    /// Subscribes, then copies the current state under the same lock, so the
    /// viewer misses no patch set newer than what it starts from.
    pub fn subscribe(&self) -> (broadcast::Receiver<Arc<PatchSet>>, Snapshot) {
        let state = self.0.state.lock().expect("hub lock");
        (self.0.tx.subscribe(), snapshot_of(&state))
    }

    /// How long each change took to render, in microseconds (both grains).
    pub fn render_us(&self) -> Vec<u64> {
        self.0.state.lock().expect("hub lock").render_us.clone()
    }

    /// The whole document as one region patch.
    pub fn full(&self) -> Arc<PatchSet> {
        let state = self.0.state.lock().expect("hub lock");
        Arc::new(state.renderer.full(unix_ms()))
    }
}

fn snapshot_of(state: &State) -> Snapshot {
    let full = state.renderer.full(state.sent_ms);
    let region = state.renderer.region();
    Snapshot {
        region_html: if region.is_empty() {
            EMPTY_REGION.to_string()
        } else {
            region.to_string()
        },
        snapshot: full.snapshot,
        connected: state.connected,
        seq: full.seq,
        sent_ms: state.sent_ms,
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
