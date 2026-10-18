//! Finding: Yrs 0.28.0 can leave a pending update unintegrated after every
//! update it depends on has arrived. Found by fuzz seed 0. Xmesh::apply works
//! around it by retrying updates that left the document with missing
//! dependencies; this test pins the raw Yrs behaviour so a fix upstream is noticed.

use xmesh::vector::replay;
use yrs::updates::decoder::Decode;
use yrs::{ReadTxn, Transact};

/// Indexes into [seed, updates...] of fuzz seed 0, in the order that sticks.
const STUCK_ORDER: [usize; 8] = [6, 7, 0, 3, 14, 5, 1, 8];

fn deliver(all: &[Vec<u8>], order: impl Iterator<Item = usize>) -> bool {
    let doc = yrs::Doc::with_options(yrs::Options {
        skip_gc: true,
        ..yrs::Options::with_client_id(yrs::block::ClientID::new(0xF000_0000))
    });
    for i in order {
        doc.transact_mut()
            .apply_update(yrs::Update::decode_v1(&all[i]).unwrap())
            .unwrap();
    }
    let missing = doc.transact().has_missing_updates();
    missing
}

#[test]
fn finding_yrs_028_leaves_a_pending_update_stuck() {
    let r = replay(&xtxp_fuzzer::generate(0)).unwrap();
    let mut all = vec![r.seed.clone()];
    all.extend(r.updates.iter().cloned());
    let mut sorted = STUCK_ORDER;
    sorted.sort();
    assert!(
        !deliver(&all, sorted.into_iter()),
        "in replay order nothing is missing"
    );
    assert!(
        deliver(&all, STUCK_ORDER.into_iter()),
        "Yrs no longer leaves the update stuck: the finding is gone, update the spec and consider dropping Xmesh's retry"
    );
}

#[test]
fn xmesh_heals_the_stuck_order() {
    let r = replay(&xtxp_fuzzer::generate(0)).unwrap();
    let mut all = vec![r.seed.clone()];
    all.extend(r.updates.iter().cloned());
    let mut sorted = STUCK_ORDER;
    sorted.sort();
    let (a, b) = (
        xmesh::Xmesh::new(0xF000_0000).unwrap(),
        xmesh::Xmesh::new(0xF000_0001).unwrap(),
    );
    for i in STUCK_ORDER {
        a.apply(&all[i]).unwrap();
    }
    for i in sorted {
        b.apply(&all[i]).unwrap();
    }
    assert_eq!(a.canonical(), b.canonical());
    assert!(!a.has_missing_updates());
}
