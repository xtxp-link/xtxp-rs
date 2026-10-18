//! The muppet's application service: a synchronous core driven by adapters.

use crate::ports::{Clock, IdSource, Outcome, Screen, Transport, View};
use xmesh::Op;
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
}
