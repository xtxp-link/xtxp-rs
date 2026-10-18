//! An in-process stand-in for y-sweet v0.9.1's message handling
//! (`y-sweet-core/src/doc_connection.rs`): on connect it sends its state
//! vector and an awareness message; it answers sync step 1 with step 2,
//! applies step 2 and updates and relays them to every other connection,
//! echoes custom message 102, and ignores awareness.

use crate::{decode, encode, SYNC_STATUS};
use std::collections::BTreeMap;
use xmesh::Xmesh;
use yrs::sync::{AwarenessUpdate, Message, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::StateVector;

pub type ConnId = u32;

/// Client ID the fake host's own document uses; it never edits.
const HOST_CLIENT: u64 = u32::MAX as u64 - 7;

pub struct FakeHost {
    doc: Xmesh,
    conns: BTreeMap<ConnId, ()>,
    next: ConnId,
}

impl Default for FakeHost {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeHost {
    pub fn new() -> FakeHost {
        FakeHost {
            doc: Xmesh::new(HOST_CLIENT).expect("valid host client ID"),
            conns: BTreeMap::new(),
            next: 1,
        }
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
