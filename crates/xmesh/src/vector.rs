//! Test vectors: the contract every implementation answers to.

use crate::{Error, Op, Snapshot, Xmesh};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One vector: a seed document, peers with fixed client IDs, and rounds of
/// edits. Within a round every peer edits its own replica concurrently;
/// between rounds every peer receives every update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vector {
    pub name: String,
    pub description: String,
    pub seed: String,
    pub peers: BTreeMap<String, u64>,
    /// Each round maps a peer to its batches; each batch is one transaction.
    pub rounds: Vec<BTreeMap<String, Vec<Vec<Op>>>>,
    #[serde(default)]
    pub expect: Expect,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Expect {
    /// The exact canonical XML every replica must end with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<String>,
    /// An expected failure: replicas converge, but not to the ideal outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_gap: Option<KnownGap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotExpect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<ForkExpect>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnownGap {
    pub reason: String,
    pub ideal_canonical: String,
}

/// The snapshot hash after round `changes_after_round` differs from the one before it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotExpect {
    pub changes_after_round: usize,
}

/// A document rebuilt from the snapshot after round `at_round` has this canonical XML.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForkExpect {
    pub at_round: usize,
    pub canonical: String,
}

impl Vector {
    pub fn from_json(json: &str) -> Result<Vector, Error> {
        serde_json::from_str(json).map_err(|e| Error::Vector(format!("bad vector JSON: {e}")))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("vectors always serialise")
    }
}

/// Every update a vector produces, and the snapshots along the way.
pub struct Replay {
    pub seed: Vec<u8>,
    pub updates: Vec<Vec<u8>>,
    /// `snapshots[0]` is after the seed, `snapshots[n]` after round `n`.
    pub snapshots: Vec<Snapshot>,
}

/// Runs a vector's rounds, collecting the updates each batch produces.
pub fn replay(v: &Vector) -> Result<Replay, Error> {
    let fail = |msg: String| Error::Vector(format!("{}: {msg}", v.name));
    let seed = Xmesh::seed(&v.seed).map_err(|e| fail(format!("seed: {e}")))?;
    let mut replicas = BTreeMap::new();
    for (name, &client) in &v.peers {
        let replica = Xmesh::new(client).map_err(|e| fail(format!("peer {name}: {e}")))?;
        replica.apply(&seed)?;
        replicas.insert(name.as_str(), replica);
    }
    let first = replicas
        .values()
        .next()
        .ok_or_else(|| fail("no peers".into()))?;
    let mut snapshots = vec![first.snapshot()];
    let mut updates = Vec::new();
    for (r, round) in v.rounds.iter().enumerate() {
        for (peer, batches) in round {
            let replica = replicas
                .get(peer.as_str())
                .ok_or_else(|| fail(format!("round {}: unknown peer {peer:?}", r + 1)))?;
            for (b, batch) in batches.iter().enumerate() {
                let update = replica
                    .edit(batch)
                    .map_err(|e| fail(format!("round {}, peer {peer}, batch {b}: {e}", r + 1)))?;
                updates.push(update);
            }
        }
        for a in replicas.values() {
            for b in replicas.values() {
                b.apply(&a.diff(&b.state_vector())?)?;
            }
        }
        snapshots.push(replicas.values().next().expect("checked above").snapshot());
    }
    Ok(Replay {
        seed,
        updates,
        snapshots,
    })
}

/// Client ID for the fresh replicas `check` delivers to; above any peer's.
const CHECKER_CLIENT: u64 = 0xF000_0000;

/// Replays `v`, delivers its updates to fresh replicas in `orders` shuffled
/// orders (some updates twice), and checks that every replica ends with the
/// same document and snapshot hashes, that re-delivery changes nothing, and
/// that the vector's expectations hold.
pub fn check(v: &Vector, orders: usize, rng_seed: u64) -> Result<(), Error> {
    let fail = |msg: String| Error::Vector(format!("{}: {msg}", v.name));
    let replay = replay(v)?;
    let mut all = vec![replay.seed.clone()];
    all.extend(replay.updates.iter().cloned());

    let reference = Xmesh::new(CHECKER_CLIENT)?;
    for u in &all {
        reference.apply(u)?;
    }
    let (hash, snapshot_hash) = (reference.hash(), reference.snapshot_hash());

    let mut rng = StdRng::seed_from_u64(rng_seed);
    for order in 0..orders {
        let mut delivery: Vec<&Vec<u8>> = Vec::new();
        for u in &all {
            delivery.push(u);
            if rng.gen_bool(0.2) {
                delivery.push(u);
            }
        }
        delivery.shuffle(&mut rng);
        let replica = Xmesh::new(CHECKER_CLIENT + 1 + order as u64)?;
        for u in &delivery {
            replica.apply(u)?;
        }
        let at = |what: &str| fail(format!("{what} (order {order}, rng seed {rng_seed})"));
        if replica.hash() != hash {
            return Err(at(&format!(
                "diverged: {} instead of {}",
                replica.canonical(),
                reference.canonical()
            )));
        }
        if replica.snapshot_hash() != snapshot_hash {
            return Err(at("snapshot hash differs from the reference"));
        }
        for u in &all {
            replica.apply(u)?;
        }
        if replica.hash() != hash || replica.snapshot_hash() != snapshot_hash {
            return Err(at("re-delivery changed the document"));
        }
    }

    let canonical = reference.canonical();
    if let Some(expected) = &v.expect.canonical {
        if &canonical != expected {
            return Err(fail(format!("expected {expected}\n  got      {canonical}")));
        }
    }
    if let Some(gap) = &v.expect.known_gap {
        if canonical == gap.ideal_canonical {
            return Err(fail(format!(
                "known gap closed ({}): result now matches the ideal; update the vector",
                gap.reason
            )));
        }
    }
    if let Some(s) = &v.expect.snapshot {
        let n = s.changes_after_round;
        if n == 0 || n >= replay.snapshots.len() {
            return Err(fail(format!(
                "snapshot.changes_after_round {n} is not a round"
            )));
        }
        if replay.snapshots[n].hash() == replay.snapshots[n - 1].hash() {
            return Err(fail(format!("snapshot hash did not change in round {n}")));
        }
    }
    if let Some(f) = &v.expect.fork {
        let snap = replay
            .snapshots
            .get(f.at_round)
            .ok_or_else(|| fail(format!("fork.at_round {} is not a round", f.at_round)))?;
        let fork = Xmesh::new(CHECKER_CLIENT)?;
        fork.apply(&reference.state_at(snap)?)?;
        let got = fork.canonical();
        if got != f.canonical {
            return Err(fail(format!(
                "fork expected {}\n  got           {got}",
                f.canonical
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../../../vectors/concurrent-attr-same-key.json");

    fn example() -> Vector {
        Vector::from_json(EXAMPLE).unwrap()
    }

    #[test]
    fn replay_counts_updates() {
        let mut v = example();
        v.rounds.push(BTreeMap::from([(
            "a".to_string(),
            vec![vec![Op::RemoveAttr {
                path: vec![0],
                name: "r".into(),
            }]],
        )]));
        let replay = replay(&v).unwrap();
        assert_eq!(replay.updates.len(), 3);
        assert_eq!(
            replay.snapshots.len(),
            3,
            "after the seed and after each of two rounds"
        );
    }

    #[test]
    fn rounds_are_concurrent_within_and_sequential_across() {
        // In round 2, b edits the circle a inserted in round 1: only possible
        // if rounds sync between them.
        let v = Vector::from_json(
            r#"{"name":"x","description":"","seed":"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
                "peers":{"a":1,"b":2},
                "rounds":[
                  {"a":[[{"op":"insert","parent":[],"index":0,"xml":"<circle/>"}]]},
                  {"b":[[{"op":"set_attr","path":[0],"name":"r","value":"2"}]]}
                ],
                "expect":{"canonical":"<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"2\"/></svg>"}}"#,
        )
        .unwrap();
        check(&v, 20, 0).unwrap();
    }

    #[test]
    fn check_passes_example() {
        check(&example(), 50, 0).unwrap();
    }

    #[test]
    fn check_reports_wrong_canonical() {
        let mut v = example();
        v.expect.canonical = Some("<svg/>".into());
        let err = check(&v, 5, 0).unwrap_err();
        assert!(
            matches!(&err, Error::Vector(m) if m.contains("concurrent-attr-same-key")),
            "{err}"
        );
    }

    #[test]
    fn known_gap_that_closes_fails() {
        let mut v = example();
        let real = v.expect.canonical.take().unwrap();
        v.expect.known_gap = Some(KnownGap {
            reason: "test".into(),
            ideal_canonical: real,
        });
        assert!(matches!(check(&v, 5, 0), Err(Error::Vector(_))));
    }

    #[test]
    fn known_gap_that_holds_passes() {
        let mut v = example();
        v.expect.canonical = None;
        v.expect.known_gap = Some(KnownGap {
            reason: "test".into(),
            ideal_canonical: "<svg/>".into(),
        });
        check(&v, 5, 0).unwrap();
    }

    #[test]
    fn unknown_peer_is_error() {
        let mut v = example();
        v.peers.remove("b");
        assert!(matches!(replay(&v), Err(Error::Vector(_))));
    }

    #[test]
    fn snapshot_expectation() {
        let mut v = example();
        v.expect.snapshot = Some(SnapshotExpect {
            changes_after_round: 1,
        });
        check(&v, 5, 0).unwrap();
    }
}
