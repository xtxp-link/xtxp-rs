//! Snapshots name points in history: a state vector plus a delete set, in an
//! encoding of XTXP's own so equal states always hash the same.

use crate::canonical::hex;
use crate::{Error, Xmesh};
use sha2::{Digest, Sha256};
use std::ops::Range;
use yrs::updates::encoder::{Encoder, EncoderV1};
use yrs::{ReadTxn, Transact};

/// A point in a document's history.
#[derive(Debug, Clone)]
pub struct Snapshot(pub(crate) yrs::Snapshot);

impl Snapshot {
    /// XTXP's canonical snapshot encoding, as unsigned LEB128 numbers:
    /// the state vector's client count, then each client ID and clock sorted
    /// by client ID; then the delete set's client count, then for each client
    /// sorted by ID its range count and each range's start and length, sorted
    /// by start with overlapping or adjacent ranges merged.
    pub fn encode_canonical(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut clocks: Vec<(u64, u32)> = self
            .0
            .state_map
            .iter()
            .map(|(c, k)| (c.get(), *k))
            .collect();
        clocks.sort();
        leb128(&mut out, clocks.len() as u64);
        for (client, clock) in clocks {
            leb128(&mut out, client);
            leb128(&mut out, clock as u64);
        }
        let mut deletes: Vec<(u64, Vec<Range<u32>>)> = self
            .0
            .delete_set
            .iter()
            .map(|(c, ranges)| (c.get(), merge(ranges.iter().cloned().collect())))
            .filter(|(_, ranges)| !ranges.is_empty())
            .collect();
        deletes.sort_by_key(|(c, _)| *c);
        leb128(&mut out, deletes.len() as u64);
        for (client, ranges) in deletes {
            leb128(&mut out, client);
            leb128(&mut out, ranges.len() as u64);
            for r in ranges {
                leb128(&mut out, r.start as u64);
                leb128(&mut out, (r.end - r.start) as u64);
            }
        }
        out
    }

    /// The first 16 lowercase hex characters of the SHA-256 of [`Snapshot::encode_canonical`].
    pub fn hash(&self) -> String {
        hex(&Sha256::digest(self.encode_canonical()))[..16].to_string()
    }
}

impl Xmesh {
    /// This replica's current point in history.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot(self.doc.transact().snapshot())
    }

    /// Shorthand for `self.snapshot().hash()`.
    pub fn snapshot_hash(&self) -> String {
        self.snapshot().hash()
    }

    /// An update that rebuilds the document as it was at `snapshot`.
    pub fn state_at(&self, snapshot: &Snapshot) -> Result<Vec<u8>, Error> {
        let mut encoder = EncoderV1::new();
        self.doc
            .transact()
            .encode_state_from_snapshot(&snapshot.0, &mut encoder)
            .map_err(|e| Error::Update(e.to_string()))?;
        Ok(encoder.to_vec())
    }
}

fn merge(mut ranges: Vec<Range<u32>>) -> Vec<Range<u32>> {
    ranges.retain(|r| r.start < r.end);
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut merged: Vec<Range<u32>> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

fn leb128(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Op;
    use yrs::block::ClientID;
    use yrs::{IdSet, StateVector, ID};

    const SVG: &str =
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect id="a"/><rect id="b"/></svg>"#;

    fn doc(client: u64) -> Xmesh {
        let doc = Xmesh::new(client).unwrap();
        doc.apply(&Xmesh::seed(SVG).unwrap()).unwrap();
        doc
    }

    #[test]
    fn encoding_layout() {
        let sv: StateVector = [(ClientID::new(5), 3), (ClientID::new(2), 7)]
            .into_iter()
            .collect();
        let mut ds = IdSet::new();
        ds.insert(ID::new(ClientID::new(5), 0), 1);
        ds.insert(ID::new(ClientID::new(5), 1), 2);
        ds.insert(ID::new(ClientID::new(2), 4), 1);
        let snap = Snapshot(yrs::Snapshot::new(sv, ds));
        assert_eq!(
            snap.encode_canonical(),
            vec![2, 2, 7, 5, 3, 2, 2, 1, 4, 1, 5, 1, 0, 3]
        );
    }

    #[test]
    fn leb128_for_large_numbers() {
        let sv: StateVector = [(ClientID::new(300), 1)].into_iter().collect();
        let snap = Snapshot(yrs::Snapshot::new(sv, IdSet::new()));
        assert_eq!(snap.encode_canonical(), vec![1, 0xac, 0x02, 1, 0]);
    }

    #[test]
    fn hash_is_16_hex() {
        let h = doc(1).snapshot_hash();
        assert_eq!(h.len(), 16);
        assert!(h
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn order_independent() {
        let peers = [doc(1), doc(2), doc(3)];
        let updates = [
            peers[0]
                .edit(&[Op::SetAttr {
                    path: vec![0],
                    name: "fill".into(),
                    value: "red".into(),
                }])
                .unwrap(),
            peers[1].edit(&[Op::Remove { path: vec![1] }]).unwrap(),
            peers[2]
                .edit(&[Op::Insert {
                    parent: vec![],
                    index: 0,
                    xml: "<g/>".into(),
                }])
                .unwrap(),
            peers[0].edit(&[Op::Remove { path: vec![0] }]).unwrap(),
        ];
        let forward = doc(4);
        let backward = doc(5);
        for u in &updates {
            forward.apply(u).unwrap();
        }
        for u in updates.iter().rev() {
            backward.apply(u).unwrap();
        }
        assert_eq!(forward.hash(), backward.hash());
        assert_eq!(forward.snapshot_hash(), backward.snapshot_hash());
    }

    #[test]
    fn delete_changes_hash() {
        let d = doc(1);
        let before = d.snapshot_hash();
        d.edit(&[Op::Remove { path: vec![1] }]).unwrap();
        assert_ne!(d.snapshot_hash(), before);
    }

    #[test]
    fn state_at_keeps_later_deletes() {
        let d = doc(1);
        let snap = d.snapshot();
        d.edit(&[Op::Remove { path: vec![1] }]).unwrap();
        assert!(!d.canonical().contains(r#"id="b""#));
        let fork = Xmesh::new(2).unwrap();
        fork.apply(&d.state_at(&snap).unwrap()).unwrap();
        assert!(
            fork.canonical().contains(r#"id="b""#),
            "{}",
            fork.canonical()
        );
        assert_eq!(fork.snapshot_hash(), snap.hash());
    }
}
