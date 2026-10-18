use xmesh::vector::{check, replay};
use xtxp_fuzzer::generate;

#[test]
fn deterministic() {
    assert_eq!(generate(42).to_json(), generate(42).to_json());
    assert_ne!(generate(42).to_json(), generate(43).to_json());
}

#[test]
fn ops_are_valid() {
    for seed in 0..50 {
        if let Err(e) = replay(&generate(seed)) {
            panic!("seed {seed}: {e}\n{}", generate(seed).to_json());
        }
    }
}

#[test]
fn shape_is_within_bounds() {
    for seed in 0..50 {
        let v = generate(seed);
        assert!((2..=4).contains(&v.peers.len()), "seed {seed}");
        assert!((1..=5).contains(&v.rounds.len()), "seed {seed}");
        assert!(
            v.expect.canonical.is_none(),
            "generated vectors check convergence only"
        );
    }
}

#[test]
fn seeds_converge() {
    for seed in 0..500 {
        let v = generate(seed);
        if let Err(e) = check(&v, 20, seed) {
            panic!("seed {seed}: {e}\n{}", v.to_json());
        }
    }
}

/// `generate` must not change: part 1's pinned seed-0 finding depends on it.
#[test]
fn seed_0_is_unchanged() {
    assert_eq!(
        xmesh::hash_of(&generate(0).to_json()),
        "49fa252cfd0ea834d1ab8f11ee57c347dd455ba3ec8ced3dacd184a34e5d798c"
    );
}

#[test]
fn single_round_shape() {
    for seed in 0..50 {
        let v = xtxp_fuzzer::generate_single_round(seed);
        assert_eq!(v.peers.len(), 3, "seed {seed}");
        assert_eq!(v.rounds.len(), 1, "seed {seed}");
        for (peer, batches) in &v.rounds[0] {
            assert!((2..=6).contains(&batches.len()), "seed {seed} peer {peer}");
        }
        assert_eq!(v.rounds[0].len(), 3, "every peer edits");
    }
}

#[test]
fn single_round_seeds_converge() {
    for seed in 0..100 {
        let v = xtxp_fuzzer::generate_single_round(seed);
        if let Err(e) = check(&v, 20, seed) {
            panic!("seed {seed}: {e}\n{}", v.to_json());
        }
    }
}
