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
