//! The muppet's application service: a synchronous core driven by adapters.

use crate::ports::{Clock, IdSource, Outcome, Screen, Transport, View};
use std::collections::BTreeMap;
use xmesh::{IdNode, IdNodeKind, NodeId, Op};
use xtxp_wire::{Event, Input, Output, Peer};

/// What the muppet is for. One-shot modes finish; `Interactive` never does.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Interactive,
    /// Apply one batch, then finish once the host has it.
    Edit(Vec<Op>),
    /// Sync, then finish.
    Show,
    /// Apply `batches[k]` once `sum(gaps_ms[..=k])` has passed, connected or
    /// not; finish when all are acknowledged and 500 ms pass without a remote update.
    Replay {
        batches: Vec<Vec<Op>>,
        gaps_ms: Vec<u64>,
    },
    /// Push a seed update into an empty document; refuse one with content.
    Init {
        seed: Vec<u8>,
    },
}

/// An edit as a person makes it: aimed at a node, not a path (P7). The node
/// is resolved to its current path when the command is handled.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    SetAttr {
        node: NodeId,
        name: String,
        value: String,
    },
    RemoveAttr {
        node: NodeId,
        name: String,
    },
    SetText {
        node: NodeId,
        text: String,
    },
    Delete {
        node: NodeId,
    },
    /// Inserts a one-line XML fragment as the element's first child.
    InsertChild {
        parent: NodeId,
        xml: String,
    },
    /// Moves the node to be `new_parent`'s first child.
    Move {
        node: NodeId,
        new_parent: NodeId,
    },
    /// The snapshot the person was looking at when they acted (P6).
    Seen {
        snapshot: String,
    },
}

/// The ops that turn `old` text into `new`: at most one delete and one insert,
/// around their common prefix and suffix, at UTF-16 offsets (S10).
pub fn text_ops(path: Vec<u32>, old: &str, new: &str) -> Vec<Op> {
    let (o, n): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let prefix = o.iter().zip(&n).take_while(|(a, b)| a == b).count();
    let max_suffix = o.len().min(n.len()) - prefix;
    let suffix = o
        .iter()
        .rev()
        .zip(n.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let units = |cs: &[char]| cs.iter().map(|c| c.len_utf16() as u32).sum::<u32>();
    let offset = units(&o[..prefix]);
    let removed = &o[prefix..o.len() - suffix];
    let added: String = n[prefix..n.len() - suffix].iter().collect();
    let mut ops = Vec::new();
    if !removed.is_empty() {
        ops.push(Op::DeleteText {
            path: path.clone(),
            offset,
            len: units(removed),
        });
    }
    if !added.is_empty() {
        ops.push(Op::InsertText {
            path,
            offset,
            text: added,
        });
    }
    ops
}

const BACKOFF_START_MS: u64 = 100;
const BACKOFF_CAP_MS: u64 = 5_000;
const ONE_SHOT_TIMEOUT_MS: u64 = 10_000;
const QUIET_MS: u64 = 500;

pub struct Muppet<T: Transport, C: Clock, I: IdSource, V: View> {
    mode: Mode,
    peer: Peer,
    transport: T,
    clock: C,
    ids: I,
    view: V,
    screen: Screen,
    backoff_ms: u64,
    reconnect_at: Option<u64>,
    /// The client ID the next connection will use, drawn when the last one dropped.
    next_id: Option<u64>,
    started_ms: u64,
    ever_synced: bool,
    updates_before_synced: u32,
    applied: bool,
    next_batch: usize,
    last_activity_ms: u64,
    finished: bool,
    seen: Option<String>,
    previous: BTreeMap<NodeId, String>,
}

impl<T: Transport, C: Clock, I: IdSource, V: View> Muppet<T, C, I, V> {
    /// A muppet in `mode`; asks the transport for a connection straight away.
    pub fn new(mode: Mode, mut transport: T, clock: C, mut ids: I, view: V) -> Self {
        let peer = Peer::new(ids.next_client_id()).expect("IdSource gives IDs in 1..2^53");
        transport.connect();
        let now = clock.now_ms();
        Muppet {
            mode,
            peer,
            transport,
            clock,
            ids,
            view,
            screen: Screen::default(),
            backoff_ms: BACKOFF_START_MS,
            reconnect_at: None,
            next_id: None,
            started_ms: now,
            ever_synced: false,
            updates_before_synced: 0,
            applied: false,
            next_batch: 0,
            last_activity_ms: now,
            finished: false,
            seen: None,
            previous: BTreeMap::new(),
        }
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    pub fn doc(&self) -> &xmesh::Xmesh {
        self.peer.doc()
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Handles a person's edit. Commands are never refused for a changed
    /// tree (P6): they apply to the current tree, with a note if it changed
    /// since the person last saw it.
    pub fn command(&mut self, command: Command) {
        let ops = match command {
            Command::Seen { snapshot } => {
                self.seen = Some(snapshot);
                return;
            }
            Command::SetAttr { node, name, value } => self
                .at(node)
                .map(|path| vec![Op::SetAttr { path, name, value }]),
            Command::RemoveAttr { node, name } => self
                .at(node)
                .map(|path| vec![Op::RemoveAttr { path, name }]),
            Command::Delete { node } => self.at(node).map(|path| vec![Op::Remove { path }]),
            Command::InsertChild { parent, xml } => self.at(parent).map(|parent| {
                vec![Op::Insert {
                    parent,
                    index: 0,
                    xml,
                }]
            }),
            Command::Move { node, new_parent } => self.at(node).and_then(|path| {
                self.at(new_parent).map(|np| {
                    vec![Op::Move {
                        path,
                        new_parent: np,
                        index: 0,
                    }]
                })
            }),
            Command::SetText { node, text } => self.at(node).map(|path| {
                let old = text_of(&self.peer.doc().tree_with_ids(), node).unwrap_or_default();
                text_ops(path, &old, &text)
            }),
        };
        let Some(ops) = ops else { return };
        let current = self.peer.doc().snapshot_hash();
        if let Some(seen) = self.seen.take() {
            if seen != current {
                let note = format!(
                    "tree changed since shown (@{} → @{}); applied to the current tree",
                    &seen[..seen.len().min(16)],
                    &current[..current.len().min(16)]
                );
                self.view.status(&note);
            }
        }
        if !ops.is_empty() {
            self.apply_edit(ops);
        }
    }

    /// The node's current path, or a status line if it's gone.
    fn at(&mut self, node: NodeId) -> Option<Vec<u32>> {
        let path = self.peer.doc().path_of(node);
        if path.is_none() {
            self.view.status(&format!(
                "node {}:{} was deleted; edit dropped",
                node.client, node.clock
            ));
        }
        path
    }

    pub fn on_connected(&mut self) {
        let out = self.peer.handle(Input::Connected);
        self.screen.connected = true;
        self.dispatch(out);
        self.refresh();
    }

    pub fn on_frame(&mut self, frame: Vec<u8>) {
        let out = self.peer.handle(Input::Frame(frame));
        self.dispatch(out);
    }

    /// The connection is gone, or never came up: schedule a reconnect.
    pub fn on_disconnected(&mut self) {
        self.peer.handle(Input::Disconnected);
        self.screen.connected = false;
        if self.finished {
            return;
        }
        if self.reconnect_at.is_none() {
            let next_id = self.ids.next_client_id();
            // Jitter of 0 to 20% comes from the fresh client ID, so the core needs no RNG.
            let jitter = self.backoff_ms * (next_id % 21) / 100;
            self.reconnect_at = Some(self.clock.now_ms() + self.backoff_ms + jitter);
            self.backoff_ms = (self.backoff_ms * 2).min(BACKOFF_CAP_MS);
            self.next_id = Some(next_id);
        }
        self.refresh();
    }

    /// Called by adapters at least every 50 ms.
    pub fn on_tick(&mut self) {
        if self.finished {
            return;
        }
        let now = self.clock.now_ms();
        if self.reconnect_at.is_some_and(|at| now >= at) {
            self.reconnect_at = None;
            // A fresh client ID for the new connection (P1); edits made while
            // offline keep the old one and go up in the next step 2.
            if let Some(id) = self.next_id.take() {
                let out = self.peer.handle(Input::Reconnect(id));
                self.dispatch(out);
            }
            self.transport.connect();
        }
        self.progress();
    }

    fn dispatch(&mut self, outputs: Vec<Output>) {
        for output in outputs {
            match output {
                Output::Send(frame) => self.transport.send(frame),
                Output::Event(event) => self.event(event),
            }
        }
        self.progress();
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::RemoteUpdate { .. } => {
                self.last_activity_ms = self.clock.now_ms();
                self.refresh();
            }
            Event::Synced => {
                self.backoff_ms = BACKOFF_START_MS;
                if !self.ever_synced {
                    self.ever_synced = true;
                    self.updates_before_synced = self.peer.updates_before_synced();
                    self.last_activity_ms = self.clock.now_ms();
                }
                self.refresh();
            }
            Event::LocalAcked => {}
            Event::EditFailed(e) => {
                let message = e.to_string();
                self.view.status(&format!("edit refused: {message}"));
                if !matches!(self.mode, Mode::Interactive) {
                    self.finish(false, Some(message));
                }
            }
            Event::ProtocolError(reason) => {
                self.view
                    .status(&format!("protocol error, reconnecting: {reason}"));
                self.transport.close();
            }
        }
    }

    /// Moves the mode forward; called after every input.
    fn progress(&mut self) {
        if self.finished {
            return;
        }
        let now = self.clock.now_ms();
        let synced = self.peer.is_synced();
        match self.mode.clone() {
            Mode::Interactive => {}
            Mode::Show => {
                if synced {
                    self.finish(true, None);
                }
            }
            Mode::Edit(ops) => {
                if synced && !self.applied {
                    self.applied = true;
                    self.apply_edit(ops);
                } else if self.applied && synced && !self.peer.unacked() {
                    self.finish(true, None);
                }
            }
            Mode::Init { seed } => {
                if synced && !self.applied {
                    if !self.peer.doc().tree().is_empty() {
                        return self.finish(
                            false,
                            Some("document already has content; init refused".into()),
                        );
                    }
                    self.applied = true;
                    let out = self.peer.handle(Input::Apply(seed));
                    self.dispatch(out);
                    self.refresh();
                } else if self.applied && synced && !self.peer.unacked() {
                    self.finish(true, None);
                }
            }
            Mode::Replay { batches, gaps_ms } => {
                while self.next_batch < batches.len()
                    && now
                        >= self.started_ms + gaps_ms.iter().take(self.next_batch + 1).sum::<u64>()
                {
                    let batch = batches[self.next_batch].clone();
                    self.next_batch += 1;
                    self.last_activity_ms = now;
                    self.apply_edit(batch);
                    if self.finished {
                        return;
                    }
                }
                if self.next_batch == batches.len()
                    && synced
                    && !self.peer.unacked()
                    && now >= self.last_activity_ms + QUIET_MS
                {
                    self.finish(true, None);
                }
            }
        }
        if !self.finished
            && !self.ever_synced
            && !matches!(self.mode, Mode::Interactive)
            && now >= self.started_ms + ONE_SHOT_TIMEOUT_MS
        {
            self.finish(false, Some("could not reach y-sweet within 10 s".into()));
        }
    }

    fn apply_edit(&mut self, ops: Vec<Op>) {
        let out = self.peer.handle(Input::Edit(ops));
        self.dispatch(out);
        self.refresh();
    }

    fn refresh(&mut self) {
        let doc = self.peer.doc();
        self.screen.tree = doc.tree_with_ids();
        let mut now = BTreeMap::new();
        fingerprint(&self.screen.tree, &mut now);
        self.screen.changed = now
            .iter()
            .filter(|(id, content)| self.previous.get(id).is_some_and(|old| old != *content))
            .map(|(id, _)| *id)
            .collect();
        self.previous = now;
        self.screen.snapshot_hash = doc.snapshot_hash();
        self.screen.doc_hash = doc.hash();
        self.view.show(&self.screen);
    }

    fn finish(&mut self, ok: bool, message: Option<String>) {
        self.finished = true;
        let doc = self.peer.doc();
        let outcome = Outcome {
            ok,
            doc_hash: doc.hash(),
            snapshot_hash: doc.snapshot_hash(),
            updates_before_synced: self.updates_before_synced,
            canonical: doc.canonical(),
            message,
        };
        self.view.finished(&outcome);
        self.transport.close();
    }
}

/// Each node's own content (name and attributes, or text), by ID, to tell which changed.
fn fingerprint(nodes: &[IdNode], out: &mut BTreeMap<NodeId, String>) {
    for n in nodes {
        match &n.node {
            IdNodeKind::Element {
                name,
                attrs,
                children,
            } => {
                out.insert(n.id, format!("{name}{attrs:?}"));
                fingerprint(children, out);
            }
            IdNodeKind::Text(t) => {
                out.insert(n.id, t.clone());
            }
        }
    }
}

fn text_of(nodes: &[IdNode], id: NodeId) -> Option<String> {
    for n in nodes {
        match &n.node {
            IdNodeKind::Text(t) if n.id == id => return Some(t.clone()),
            IdNodeKind::Element { children, .. } => {
                if let Some(t) = text_of(children, id) {
                    return Some(t);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod testkit;

#[cfg(test)]
mod tests {
    use super::testkit::World;
    use super::*;

    const SVG: &str =
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect id="a"/><circle id="b"/></svg>"#;

    fn seeded(world: &mut World) {
        let m = world.add(Mode::Init {
            seed: xmesh::Xmesh::seed(SVG).unwrap(),
        });
        world.run_until_finished(m);
        assert!(world.outcome(m).unwrap().ok);
    }

    fn fill(path: Vec<u32>, v: &str) -> Op {
        Op::SetAttr {
            path,
            name: "fill".into(),
            value: v.into(),
        }
    }

    #[test]
    fn two_muppets_converge() {
        let mut w = World::new();
        seeded(&mut w);
        let a = w.add(Mode::Interactive);
        let b = w.add(Mode::Interactive);
        w.pump();
        let e1 = w.add(Mode::Edit(vec![fill(vec![0], "red")]));
        w.run_until_finished(e1);
        w.pump();
        assert_eq!(w.muppet(a).screen().doc_hash, w.muppet(b).screen().doc_hash);
        assert!(w.muppet(a).doc().canonical().contains(r#"fill="red""#));
        assert!(
            w.shown(b) >= 2,
            "b's view was refreshed when the edit arrived"
        );
    }

    #[test]
    fn edit_mode_finishes_after_ack() {
        let mut w = World::new();
        seeded(&mut w);
        let e = w.add(Mode::Edit(vec![fill(vec![1], "blue")]));
        w.run_until_finished(e);
        let out = w.outcome(e).unwrap();
        assert!(out.ok, "{out:?}");
        assert!(out.canonical.contains(r#"fill="blue""#));
        assert_eq!(out.doc_hash, w.host_hash());
    }

    #[test]
    fn edit_mode_reports_a_failing_op() {
        let mut w = World::new();
        seeded(&mut w);
        let e = w.add(Mode::Edit(vec![Op::Remove { path: vec![9] }]));
        w.run_until_finished(e);
        let out = w.outcome(e).unwrap();
        assert!(!out.ok);
        assert!(out.message.unwrap().contains("[9]"));
    }

    #[test]
    fn show_mode_reports_updates_before_synced() {
        let mut w = World::new();
        seeded(&mut w);
        let s = w.add(Mode::Show);
        w.run_until_finished(s);
        let out = w.outcome(s).unwrap();
        assert!(out.ok);
        assert_eq!(out.updates_before_synced, 1);
        assert_eq!(out.doc_hash, w.host_hash());
    }

    #[test]
    fn init_refuses_existing_content() {
        let mut w = World::new();
        seeded(&mut w);
        let before = w.host_hash();
        let again = w.add(Mode::Init {
            seed: xmesh::Xmesh::seed("<other/>").unwrap(),
        });
        w.run_until_finished(again);
        let out = w.outcome(again).unwrap();
        assert!(!out.ok);
        assert!(out.message.unwrap().contains("already has content"));
        assert_eq!(w.host_hash(), before);
    }

    #[test]
    fn replay_applies_on_schedule_and_finishes_quietly() {
        let mut w = World::new();
        seeded(&mut w);
        let r = w.add(Mode::Replay {
            batches: vec![vec![fill(vec![0], "red")], vec![fill(vec![1], "blue")]],
            gaps_ms: vec![100, 200],
        });
        w.pump();
        assert!(
            !w.muppet(r).doc().canonical().contains("red"),
            "nothing before the first gap"
        );
        w.advance(100);
        assert!(w.muppet(r).doc().canonical().contains("red"));
        assert!(!w.muppet(r).doc().canonical().contains("blue"));
        w.advance(200);
        assert!(w.muppet(r).doc().canonical().contains("blue"));
        w.advance(400);
        assert!(!w.muppet(r).is_finished(), "not quiet for 500 ms yet");
        w.advance(150);
        assert!(w.muppet(r).is_finished());
        assert!(w.outcome(r).unwrap().ok);
        assert_eq!(w.outcome(r).unwrap().doc_hash, w.host_hash());
    }

    #[test]
    fn reconnects_with_backoff_and_fresh_id() {
        let mut w = World::new();
        seeded(&mut w);
        let m = w.add(Mode::Interactive);
        w.pump();
        let first = w.muppet(m).doc().client_id();
        w.drop_connection(m);
        assert!(!w.muppet(m).screen().connected);
        assert_eq!(
            w.muppet(m).doc().client_id(),
            first,
            "offline edits keep the old ID (P1)"
        );
        w.advance(99);
        assert!(!w.is_connected(m), "backoff not over at 99 ms");
        w.advance(30);
        assert!(
            w.is_connected(m),
            "reconnected after 100 ms plus up to 20% jitter"
        );
        assert_ne!(w.muppet(m).doc().client_id(), first, "fresh client ID");
        assert!(w.muppet(m).screen().connected);
    }

    #[test]
    fn backoff_doubles_until_synced() {
        let mut w = World::new();
        seeded(&mut w);
        w.refuse_connections(true);
        let m = w.add(Mode::Interactive);
        w.pump();
        // Connections fail immediately: 100, 200, 400 ms (+ up to 20%) apart.
        w.advance(125);
        let tries_after_first_gap = w.connect_attempts(m);
        w.advance(250);
        assert_eq!(w.connect_attempts(m), tries_after_first_gap + 1);
        w.advance(200);
        assert_eq!(
            w.connect_attempts(m),
            tries_after_first_gap + 1,
            "the third gap is 400 ms"
        );
    }

    #[test]
    fn one_shot_times_out() {
        let mut w = World::new();
        w.refuse_connections(true);
        let s = w.add(Mode::Show);
        w.advance(9_999);
        assert!(!w.muppet(s).is_finished());
        w.advance(1);
        let out = w.outcome(s).unwrap();
        assert!(!out.ok);
        assert!(out.message.unwrap().contains("could not reach y-sweet"));
    }

    fn root_children(m: &super::testkit::TestMuppet) -> Vec<xmesh::IdNode> {
        let tree = &m.screen().tree;
        let root = tree.iter().find(|n| matches!(&n.node, xmesh::IdNodeKind::Element { name, .. } if name.ends_with("}svg"))).unwrap();
        match &root.node {
            xmesh::IdNodeKind::Element { children, .. } => children.clone(),
            _ => unreachable!(),
        }
    }

    fn interactive_pair(w: &mut World) -> (usize, usize) {
        seeded(w);
        let a = w.add(Mode::Interactive);
        let b = w.add(Mode::Interactive);
        w.pump();
        (a, b)
    }

    #[test]
    fn set_attr_on_node_after_remote_insert_above() {
        let mut w = World::new();
        let (a, b) = interactive_pair(&mut w);
        let rect = root_children(w.muppet(a))[0].id;
        let root = root_id(w.muppet(b));
        w.muppet_mut(b).command(Command::InsertChild {
            parent: root,
            xml: "<ellipse/>".into(),
        });
        w.pump();
        w.muppet_mut(a).command(Command::SetAttr {
            node: rect,
            name: "fill".into(),
            value: "red".into(),
        });
        w.pump();
        let canon = w.muppet(b).doc().canonical();
        assert!(
            canon.contains(r#"<ellipse/><rect fill="red" id="a"/>"#),
            "{canon}"
        );
    }

    fn root_id(m: &super::testkit::TestMuppet) -> xmesh::NodeId {
        m.screen().tree.iter().find(|n| matches!(&n.node, xmesh::IdNodeKind::Element { name, .. } if name.ends_with("}svg"))).unwrap().id
    }

    #[test]
    fn edit_of_node_deleted_meanwhile_is_dropped() {
        let mut w = World::new();
        let (a, b) = interactive_pair(&mut w);
        let rect = root_children(w.muppet(a))[0].id;
        w.muppet_mut(b).command(Command::Delete { node: rect });
        w.pump();
        let before = w.muppet(a).doc().hash();
        w.muppet_mut(a).command(Command::SetAttr {
            node: rect,
            name: "fill".into(),
            value: "red".into(),
        });
        w.pump();
        assert_eq!(w.muppet(a).doc().hash(), before);
        assert!(
            w.view(a).statuses.iter().any(|s| s.contains("was deleted")),
            "{:?}",
            w.view(a).statuses
        );
    }

    #[test]
    fn note_when_tree_changed_since_seen() {
        let mut w = World::new();
        let (a, b) = interactive_pair(&mut w);
        let old = w.muppet(a).screen().snapshot_hash.clone();
        w.muppet_mut(a).command(Command::Seen {
            snapshot: old.clone(),
        });
        let circle = root_children(w.muppet(b))[1].id;
        w.muppet_mut(b).command(Command::SetAttr {
            node: circle,
            name: "r".into(),
            value: "4".into(),
        });
        w.pump();
        let new = w.muppet(a).screen().snapshot_hash.clone();
        assert_ne!(old, new);
        let rect = root_children(w.muppet(a))[0].id;
        w.muppet_mut(a).command(Command::SetAttr {
            node: rect,
            name: "fill".into(),
            value: "red".into(),
        });
        w.pump();
        let note =
            format!("tree changed since shown (@{old} → @{new}); applied to the current tree");
        assert!(
            w.view(a).statuses.contains(&note),
            "{:?}",
            w.view(a).statuses
        );
        assert!(
            w.muppet(b).doc().canonical().contains(r#"fill="red""#),
            "applied anyway (P6)"
        );
    }

    #[test]
    fn no_note_when_unchanged() {
        let mut w = World::new();
        let (a, _) = interactive_pair(&mut w);
        let snap = w.muppet(a).screen().snapshot_hash.clone();
        w.muppet_mut(a).command(Command::Seen { snapshot: snap });
        let rect = root_children(w.muppet(a))[0].id;
        w.muppet_mut(a).command(Command::SetAttr {
            node: rect,
            name: "fill".into(),
            value: "red".into(),
        });
        assert!(!w
            .view(a)
            .statuses
            .iter()
            .any(|s| s.starts_with("tree changed")));
    }

    #[test]
    fn set_text_diffs_in_utf16() {
        let ops = text_ops(vec![0, 0], "héllo 😀", "héllo 😀!");
        assert_eq!(
            ops,
            vec![Op::InsertText {
                path: vec![0, 0],
                offset: 8,
                text: "!".into()
            }]
        );
        let ops = text_ops(vec![1], "abcdef", "abXYef");
        assert_eq!(
            ops,
            vec![
                Op::DeleteText {
                    path: vec![1],
                    offset: 2,
                    len: 2
                },
                Op::InsertText {
                    path: vec![1],
                    offset: 2,
                    text: "XY".into()
                }
            ]
        );
        assert!(text_ops(vec![1], "same", "same").is_empty());
    }

    #[test]
    fn set_text_edits_through_the_host() {
        let mut w = World::new();
        let m = w.add(Mode::Init {
            seed: xmesh::Xmesh::seed(
                r#"<svg xmlns="http://www.w3.org/2000/svg"><text>héllo 😀</text></svg>"#,
            )
            .unwrap(),
        });
        w.run_until_finished(m);
        let a = w.add(Mode::Interactive);
        let b = w.add(Mode::Interactive);
        w.pump();
        let text_node = match &root_children(w.muppet(a))[0].node {
            xmesh::IdNodeKind::Element { children, .. } => children[0].id,
            _ => unreachable!(),
        };
        w.muppet_mut(a).command(Command::SetText {
            node: text_node,
            text: "héllo 😀!".into(),
        });
        w.pump();
        assert!(w
            .muppet(b)
            .doc()
            .canonical()
            .contains("<text>héllo 😀!</text>"));
    }

    #[test]
    fn changed_lists_remote_edits() {
        let mut w = World::new();
        let (a, b) = interactive_pair(&mut w);
        let circle = root_children(w.muppet(a))[1].id;
        w.muppet_mut(b).command(Command::SetAttr {
            node: circle,
            name: "r".into(),
            value: "9".into(),
        });
        w.pump();
        assert_eq!(w.muppet(a).screen().changed, vec![circle]);
    }

    #[test]
    fn move_makes_first_child() {
        let mut w = World::new();
        let (a, b) = interactive_pair(&mut w);
        let kids = root_children(w.muppet(a));
        let (rect, circle) = (kids[0].id, kids[1].id);
        w.muppet_mut(a).command(Command::Move {
            node: circle,
            new_parent: rect,
        });
        w.pump();
        assert!(w
            .muppet(b)
            .doc()
            .canonical()
            .contains(r#"<rect id="a"><circle id="b"/></rect>"#));
    }
}
