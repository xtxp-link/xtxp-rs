//! The host side of y-sweet v0.9.1's message handling
//! (`y-sweet-core/src/doc_connection.rs`), as a pure state machine: on
//! connect it sends its state vector and an awareness message; it answers
//! sync step 1 with step 2, applies step 2 and updates and relays them to
//! every other connection, echoes custom message 102, and ignores awareness.
//! The tests' in-process y-sweet, and the Worker's host.

use crate::{decode, encode, SYNC_STATUS};
use std::collections::BTreeMap;
use xmesh::Xmesh;
use yrs::sync::{AwarenessUpdate, Message, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::StateVector;

pub type ConnId = u32;

/// Client ID the host's own document uses; it never edits.
const HOST_CLIENT: u64 = u32::MAX as u64 - 7;

/// An update with no structs and no deletions, in v1 encoding.
const EMPTY_UPDATE: [u8; 2] = [0, 0];

pub struct Host {
    doc: Xmesh,
    conns: BTreeMap<ConnId, ()>,
    next: ConnId,
    changed: bool,
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    pub fn new() -> Host {
        Host {
            doc: Xmesh::new(HOST_CLIENT).expect("valid host client ID"),
            conns: BTreeMap::new(),
            next: 1,
            changed: false,
        }
    }

    /// A host whose document starts from `state`, as [`Host::state`] gave it.
    pub fn load(state: &[u8]) -> Result<Host, String> {
        let host = Host::new();
        host.doc.apply(state).map_err(|e| e.to_string())?;
        Ok(host)
    }

    /// The whole document as one update.
    pub fn state(&self) -> Vec<u8> {
        use yrs::updates::encoder::Encode;
        self.doc
            .diff(&StateVector::default().encode_v1())
            .expect("empty state vector")
    }

    pub fn doc(&self) -> &Xmesh {
        &self.doc
    }

    /// Whether a peer sent anything but an empty update since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// A new connection, and the frames y-sweet sends on connect.
    pub fn connect(&mut self) -> (ConnId, Vec<Vec<u8>>) {
        let conn = self.next;
        self.next += 1;
        self.conns.insert(conn, ());
        let sv =
            StateVector::decode_v1(&self.doc.state_vector()).expect("own state vector decodes");
        let frames = vec![
            encode(&Message::Sync(SyncMessage::SyncStep1(sv))),
            encode(&Message::Awareness(AwarenessUpdate {
                clients: Default::default(),
            })),
        ];
        (conn, frames)
    }

    /// Handles one frame from `conn`; returns frames to send, by connection.
    pub fn receive(&mut self, conn: ConnId, frame: &[u8]) -> Vec<(ConnId, Vec<u8>)> {
        let Ok(message) = decode(frame) else {
            return vec![];
        };
        match message {
            Message::Sync(SyncMessage::SyncStep1(sv)) => {
                use yrs::updates::encoder::Encode;
                let diff = self
                    .doc
                    .diff(&sv.encode_v1())
                    .expect("state vector from a peer");
                vec![(conn, encode(&Message::Sync(SyncMessage::SyncStep2(diff))))]
            }
            Message::Sync(SyncMessage::SyncStep2(update))
            | Message::Sync(SyncMessage::Update(update)) => {
                if self.doc.apply(&update).is_err() {
                    return vec![];
                }
                // A peer's step 2 is empty when it has nothing new.
                self.changed |= update != EMPTY_UPDATE;
                let relay = encode(&Message::Sync(SyncMessage::Update(update)));
                self.conns
                    .keys()
                    .filter(|c| **c != conn)
                    .map(|c| (*c, relay.clone()))
                    .collect()
            }
            Message::Custom(SYNC_STATUS, data) => {
                vec![(conn, encode(&Message::Custom(SYNC_STATUS, data)))]
            }
            _ => vec![],
        }
    }

    pub fn disconnect(&mut self, conn: ConnId) {
        self.conns.remove(&conn);
    }

    pub fn doc_hash(&self) -> String {
        self.doc.hash()
    }
}
