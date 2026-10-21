//! One document's page state: the renderer, the connection state its header
//! shows, and what a new viewer starts from.

use super::dirty::{template, PatchSet, Renderer};
use super::html::state_html;
use xmesh::IdNode;

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

#[derive(Default)]
pub struct Page {
    renderer: Renderer,
    connected: bool,
    sent_ms: u64,
}

impl Page {
    pub fn new() -> Page {
        Page::default()
    }

    /// Renders a new tree and connection state; the patch set to publish, if
    /// anything changed. `now_ms` is Unix time, for `data-sent`.
    pub fn show(
        &mut self,
        tree: &[IdNode],
        snapshot: &str,
        connected: bool,
        now_ms: u64,
    ) -> Option<PatchSet> {
        let state_changed = self.connected != connected;
        self.connected = connected;
        let patch = self.renderer.update(tree, snapshot, now_ms);
        // A change of connection state patches the header's state marker,
        // with the content's patch set or on its own.
        let patch = match (patch, state_changed) {
            (Some(mut p), true) => {
                let st = template("state", p.seq, now_ms, &state_html(connected));
                p.node_html.push_str(&st);
                p.region_html.push_str(&st);
                Some(p)
            }
            (None, true) => {
                let seq = self.renderer.next_seq();
                let st = template("state", seq, now_ms, &state_html(connected));
                Some(PatchSet {
                    seq,
                    sent_ms: now_ms,
                    snapshot: snapshot.to_string(),
                    node_html: st.clone(),
                    region_html: st,
                })
            }
            (p, false) => p,
        };
        if patch.is_some() {
            self.sent_ms = now_ms;
        }
        patch
    }

    /// The current page state.
    pub fn snapshot(&self) -> Snapshot {
        let full = self.renderer.full(self.sent_ms);
        let region = self.renderer.region();
        Snapshot {
            region_html: if region.is_empty() {
                EMPTY_REGION.to_string()
            } else {
                region.to_string()
            },
            snapshot: full.snapshot,
            connected: self.connected,
            seq: full.seq,
            sent_ms: self.sent_ms,
        }
    }

    /// The whole document as one region patch.
    pub fn full(&self, now_ms: u64) -> PatchSet {
        self.renderer.full(now_ms)
    }
}
