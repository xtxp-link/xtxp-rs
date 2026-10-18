//! The peer side of a muppet's conversation with a host, over one
//! connection: the Yjs sync protocol as y-sweet v0.9.1 speaks it, as a pure
//! state machine. No I/O, no clock, no threads, no randomness: the caller
//! moves frames and supplies client IDs. Merging is Xmesh's job.

#[cfg(feature = "fake-host")]
pub mod fake_host;

use xmesh::{Op, Xmesh};
use yrs::sync::Message;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;

/// y-sweet's "sync status" custom message, echoed back in order.
pub const SYNC_STATUS: u8 = 102;

pub(crate) fn encode(message: &Message) -> Vec<u8> {
    message.encode_v1()
}

pub(crate) fn decode(frame: &[u8]) -> Result<Message, String> {
    Message::decode_v1(frame).map_err(|e| e.to_string())
}

/// What the adapter feeds the peer.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Connected,
    Disconnected,
    /// One binary WebSocket message from the host.
    Frame(Vec<u8>),
    /// A local batch, applied as one transaction.
    Edit(Vec<Op>),
    /// A raw update made elsewhere, such as a document's seed: applied
    /// locally and sent like an edit.
    Apply(Vec<u8>),
    /// Switch to a fresh client ID before the next connection (decision 19).
    Reconnect(u64),
}

/// What the peer asks the adapter to do or tells it.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Send(Vec<u8>),
    Event(Event),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The document changed because of the host; snapshot hashes before and after.
    RemoteUpdate {
        before: String,
        after: String,
    },
    /// Both directions are in step: the host's step 2 is applied and it has ours.
    Synced,
    /// The host has every local edit sent so far.
    LocalAcked,
    EditFailed(xmesh::Error),
    /// A frame couldn't be decoded or applied; the adapter should reconnect.
    ProtocolError(String),
}

/// One peer's side of one connection to a host.
pub struct Peer {
    doc: Xmesh,
    connected: bool,
    host_step2: bool,
    step2_echo: Option<u64>,
    step2_acked: bool,
    synced: bool,
    next_counter: u64,
    last_sent: Option<u64>,
    unacked: bool,
    updates_before_synced: u32,
}

impl Peer {
    pub fn new(client_id: u64) -> Result<Peer, xmesh::Error> {
        Ok(Peer {
            doc: Xmesh::new(client_id)?,
            connected: false,
            host_step2: false,
            step2_echo: None,
            step2_acked: false,
            synced: false,
            next_counter: 1,
            last_sent: None,
            unacked: false,
            updates_before_synced: 0,
        })
    }

    pub fn doc(&self) -> &Xmesh {
        &self.doc
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn is_synced(&self) -> bool {
        self.synced
    }

    /// True while a local edit hasn't been acknowledged by the host.
    pub fn unacked(&self) -> bool {
        self.unacked
    }

    /// Updates received on this connection before it was synced; a late
    /// joiner gets the whole document in one.
    pub fn updates_before_synced(&self) -> u32 {
        self.updates_before_synced
    }

    pub fn handle(&mut self, input: Input) -> Vec<Output> {
        let mut out = Vec::new();
        match input {
            Input::Connected => {
                self.reset_connection();
                self.connected = true;
                out.push(Output::Send(encode(&Message::Sync(
                    yrs::sync::SyncMessage::SyncStep1(self.state_vector()),
                ))));
            }
            Input::Disconnected => {
                self.connected = false;
                self.synced = false;
            }
            Input::Frame(frame) => self.frame(&frame, &mut out),
            Input::Edit(ops) => match self.doc.edit(&ops) {
                Ok(update) => {
                    self.unacked = true;
                    if self.connected {
                        out.push(Output::Send(encode(&Message::Sync(
                            yrs::sync::SyncMessage::Update(update),
                        ))));
                        out.push(self.status());
                    }
                }
                Err(e) => out.push(Output::Event(Event::EditFailed(e))),
            },
            Input::Apply(update) => match self.doc.apply(&update) {
                Ok(()) => {
                    self.unacked = true;
                    if self.connected {
                        out.push(Output::Send(encode(&Message::Sync(
                            yrs::sync::SyncMessage::Update(update),
                        ))));
                        out.push(self.status());
                    }
                }
                Err(e) => out.push(Output::Event(Event::EditFailed(e))),
            },
            Input::Reconnect(client_id) => match self.fresh_doc(client_id) {
                Ok(doc) => {
                    self.doc = doc;
                    self.connected = false;
                    self.reset_connection();
                }
                Err(e) => out.push(Output::Event(Event::ProtocolError(format!(
                    "reconnect: {e}"
                )))),
            },
        }
        out
    }

    fn reset_connection(&mut self) {
        self.host_step2 = false;
        self.step2_echo = None;
        self.step2_acked = false;
        self.synced = false;
        self.updates_before_synced = 0;
    }

    fn state_vector(&self) -> yrs::StateVector {
        yrs::StateVector::decode_v1(&self.doc.state_vector())
            .expect("Xmesh's own state vector decodes")
    }

    /// The whole document under a new client ID: edits keep the IDs they were made with.
    fn fresh_doc(&self, client_id: u64) -> Result<Xmesh, xmesh::Error> {
        let doc = Xmesh::new(client_id)?;
        let full = self.doc.diff(&yrs::StateVector::default().encode_v1())?;
        doc.apply(&full)?;
        Ok(doc)
    }

    /// A sync-status message carrying the next counter; the host echoes it in order.
    fn status(&mut self) -> Output {
        let counter = self.next_counter;
        self.next_counter += 1;
        self.last_sent = Some(counter);
        Output::Send(encode(&Message::Custom(
            SYNC_STATUS,
            counter.to_le_bytes().to_vec(),
        )))
    }

    fn frame(&mut self, frame: &[u8], out: &mut Vec<Output>) {
        use yrs::sync::SyncMessage;
        let message = match decode(frame) {
            Ok(m) => m,
            Err(e) => {
                return out.push(Output::Event(Event::ProtocolError(format!(
                    "undecodable frame: {e}"
                ))))
            }
        };
        match message {
            Message::Sync(SyncMessage::SyncStep1(host_sv)) => {
                let diff = match self.doc.diff(&host_sv.encode_v1()) {
                    Ok(d) => d,
                    Err(e) => return out.push(Output::Event(Event::ProtocolError(e.to_string()))),
                };
                out.push(Output::Send(encode(&Message::Sync(
                    SyncMessage::SyncStep2(diff),
                ))));
                out.push(self.status());
                self.step2_echo = self.last_sent;
            }
            Message::Sync(SyncMessage::SyncStep2(update)) => {
                self.remote(&update, out);
                self.host_step2 = true;
            }
            Message::Sync(SyncMessage::Update(update)) => self.remote(&update, out),
            Message::Custom(SYNC_STATUS, data) => {
                let counter = data
                    .get(..8)
                    .and_then(|b| b.try_into().ok())
                    .map(u64::from_le_bytes);
                if counter.is_some() && counter == self.step2_echo {
                    self.step2_acked = true;
                }
                if counter.is_some() && counter == self.last_sent && self.unacked {
                    self.unacked = false;
                    out.push(Output::Event(Event::LocalAcked));
                }
            }
            _ => {}
        }
        if self.host_step2 && self.step2_acked && !self.synced {
            self.synced = true;
            out.push(Output::Event(Event::Synced));
        }
    }

    fn remote(&mut self, update: &[u8], out: &mut Vec<Output>) {
        let before = self.doc.snapshot_hash();
        if let Err(e) = self.doc.apply(update) {
            return out.push(Output::Event(Event::ProtocolError(e.to_string())));
        }
        if !self.synced {
            self.updates_before_synced += 1;
        }
        let after = self.doc.snapshot_hash();
        if after != before {
            out.push(Output::Event(Event::RemoteUpdate { before, after }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_host::{ConnId, FakeHost};
    use std::collections::VecDeque;

    const SVG: &str =
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect id="a"/><circle id="b"/></svg>"#;

    /// Peers wired to one fake host, delivering frames until quiet.
    struct Net {
        host: FakeHost,
        peers: Vec<(Peer, Option<ConnId>)>,
        events: Vec<Vec<Event>>,
        queue: VecDeque<(usize, Vec<u8>)>,
    }

    impl Net {
        fn new() -> Net {
            Net {
                host: FakeHost::new(),
                peers: vec![],
                events: vec![],
                queue: VecDeque::new(),
            }
        }

        fn add(&mut self, client: u64) -> usize {
            self.peers.push((Peer::new(client).unwrap(), None));
            self.events.push(vec![]);
            self.peers.len() - 1
        }

        fn connect(&mut self, i: usize) {
            let (conn, frames) = self.host.connect();
            self.peers[i].1 = Some(conn);
            self.input(i, Input::Connected);
            for f in frames {
                self.input(i, Input::Frame(f));
            }
            self.settle();
        }

        fn disconnect(&mut self, i: usize) {
            if let Some(conn) = self.peers[i].1.take() {
                self.host.disconnect(conn);
            }
            self.input(i, Input::Disconnected);
        }

        fn input(&mut self, i: usize, input: Input) {
            for out in self.peers[i].0.handle(input) {
                match out {
                    Output::Send(frame) => {
                        if let Some(conn) = self.peers[i].1 {
                            for (to, f) in self.host.receive(conn, &frame) {
                                let j =
                                    self.peers.iter().position(|(_, c)| *c == Some(to)).unwrap();
                                self.queue.push_back((j, f));
                            }
                        }
                    }
                    Output::Event(e) => self.events[i].push(e),
                }
            }
        }

        fn settle(&mut self) {
            while let Some((j, f)) = self.queue.pop_front() {
                self.input(j, Input::Frame(f));
            }
        }

        fn edit(&mut self, i: usize, ops: Vec<Op>) {
            self.input(i, Input::Edit(ops));
            self.settle();
        }

        fn seed(&mut self, i: usize) {
            let seed = Xmesh::seed(SVG).unwrap();
            // A seed is a plain update; push it as an edit-free update.
            self.peers[i].0.doc().apply(&seed).unwrap();
            self.disconnect(i);
            self.connect(i);
        }
    }

    fn set_fill(path: Vec<u32>, value: &str) -> Vec<Op> {
        vec![Op::SetAttr {
            path,
            name: "fill".into(),
            value: value.into(),
        }]
    }

    #[test]
    fn two_peers_converge_through_fake_host() {
        let mut net = Net::new();
        let a = net.add(1);
        let b = net.add(2);
        net.connect(a);
        net.seed(a);
        net.connect(b);
        net.edit(a, set_fill(vec![0], "red"));
        net.edit(b, set_fill(vec![1], "blue"));
        let (ha, hb) = (net.peers[a].0.doc().hash(), net.peers[b].0.doc().hash());
        assert_eq!(ha, hb);
        assert_eq!(ha, net.host.doc_hash());
        assert!(net.peers[a].0.doc().canonical().contains(r#"fill="blue""#));
        assert!(net.events[b]
            .iter()
            .any(|e| matches!(e, Event::RemoteUpdate { .. })));
    }

    #[test]
    fn late_joiner_gets_one_update() {
        let mut net = Net::new();
        let a = net.add(1);
        net.connect(a);
        net.seed(a);
        net.edit(a, set_fill(vec![0], "red"));
        net.edit(a, set_fill(vec![1], "blue"));
        let late = net.add(3);
        net.connect(late);
        assert!(net.peers[late].0.is_synced());
        assert_eq!(net.peers[late].0.updates_before_synced(), 1);
        assert_eq!(net.peers[late].0.doc().hash(), net.peers[a].0.doc().hash());
        assert!(net.events[late].contains(&Event::Synced));
    }

    #[test]
    fn edit_is_acked() {
        let mut net = Net::new();
        let a = net.add(1);
        net.connect(a);
        net.seed(a);
        net.events[a].clear();
        net.peers[a].0.handle(Input::Edit(set_fill(vec![0], "red")));
        assert!(net.peers[a].0.unacked(), "unacked until the echo arrives");
        let mut net2 = Net::new();
        let p = net2.add(1);
        net2.connect(p);
        net2.seed(p);
        net2.events[p].clear();
        net2.edit(p, set_fill(vec![0], "red"));
        assert!(!net2.peers[p].0.unacked());
        assert!(net2.events[p].contains(&Event::LocalAcked));
    }

    #[test]
    fn offline_edit_survives_reconnect() {
        let mut net = Net::new();
        let a = net.add(1);
        let b = net.add(2);
        net.connect(a);
        net.seed(a);
        net.connect(b);
        net.disconnect(a);
        net.edit(a, set_fill(vec![0], "red"));
        net.input(a, Input::Reconnect(11));
        assert_eq!(net.peers[a].0.doc().client_id(), 11);
        net.connect(a);
        net.settle();
        assert_eq!(net.host.doc_hash(), net.peers[a].0.doc().hash());
        assert_eq!(net.peers[b].0.doc().hash(), net.peers[a].0.doc().hash());
        assert!(net.peers[b].0.doc().canonical().contains(r#"fill="red""#));
        // The offline edit kept the old client ID; new edits use the new one.
        net.edit(a, set_fill(vec![1], "blue"));
        assert_eq!(net.peers[b].0.doc().hash(), net.peers[a].0.doc().hash());
    }

    #[test]
    fn duplicate_frames_change_nothing() {
        let mut a = Peer::new(1).unwrap();
        let seed = Xmesh::seed(SVG).unwrap();
        a.handle(Input::Connected);
        let frame = encode(&Message::Sync(yrs::sync::SyncMessage::Update(seed)));
        a.handle(Input::Frame(frame.clone()));
        let h = a.doc().hash();
        let again = a.handle(Input::Frame(frame));
        assert_eq!(a.doc().hash(), h);
        assert!(!again
            .iter()
            .any(|o| matches!(o, Output::Event(Event::RemoteUpdate { .. }))));
    }

    #[test]
    fn garbage_frame_is_protocol_error() {
        let mut a = Peer::new(1).unwrap();
        a.handle(Input::Connected);
        let out = a.handle(Input::Frame(vec![0xff, 0xff, 0xff]));
        assert!(out
            .iter()
            .any(|o| matches!(o, Output::Event(Event::ProtocolError(_)))));
    }

    #[test]
    fn unknown_messages_ignored() {
        let mut a = Peer::new(1).unwrap();
        a.handle(Input::Connected);
        for frame in [
            encode(&Message::Awareness(yrs::sync::AwarenessUpdate {
                clients: Default::default(),
            })),
            encode(&Message::Auth(Some("denied".into()))),
            encode(&Message::Custom(7, vec![1, 2, 3])),
            encode(&Message::AwarenessQuery),
        ] {
            let out = a.handle(Input::Frame(frame));
            assert!(out.is_empty(), "{out:?}");
        }
    }

    #[test]
    fn edit_failure_reported() {
        let mut a = Peer::new(1).unwrap();
        let out = a.handle(Input::Edit(vec![Op::Remove { path: vec![9] }]));
        assert!(out
            .iter()
            .any(|o| matches!(o, Output::Event(Event::EditFailed(_)))));
    }

    #[test]
    fn apply_input_pushes_a_raw_update() {
        let mut net = Net::new();
        let a = net.add(1);
        let b = net.add(2);
        net.connect(a);
        net.connect(b);
        net.input(a, Input::Apply(Xmesh::seed(SVG).unwrap()));
        net.settle();
        assert!(!net.peers[a].0.unacked());
        assert_eq!(net.peers[b].0.doc().hash(), net.peers[a].0.doc().hash());
        assert!(net.peers[b].0.doc().canonical().contains("circle"));
        assert!(net.events[a].contains(&Event::LocalAcked));
    }
}
