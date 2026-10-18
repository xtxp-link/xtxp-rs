//! End-to-end: muppet processes through the chaos proxy against a real y-sweet.
//! Feature files: features/late-joiner.feature, features/reconnect.feature.

mod support;

use chaos_proxy::Chaos;
use std::path::Path;
use support::{hashes, muppet, spawn_muppet, Proxy, YSweet};
use xmesh::vector::{replay, Vector};
use xmesh::Xmesh;

/// The hashes the vector produces in-process: what every peer must reach.
fn reference(v: &Vector) -> (String, String) {
    let r = replay(v).unwrap();
    let doc = Xmesh::new(0xF000_0000).unwrap();
    doc.apply(&r.seed).unwrap();
    for u in &r.updates {
        doc.apply(u).unwrap();
    }
    (doc.hash(), doc.snapshot_hash())
}

/// Writes the vector and its seed drawing to `dir`; returns their paths.
fn files(dir: &Path, v: &Vector) -> (String, String) {
    let vector = dir.join(format!("{}.json", v.name));
    let seed = dir.join(format!("{}.svg", v.name));
    std::fs::write(&vector, v.to_json()).unwrap();
    std::fs::write(&seed, &v.seed).unwrap();
    (
        vector.to_string_lossy().into_owned(),
        seed.to_string_lossy().into_owned(),
    )
}

/// Seeds the document directly (no chaos), then runs every peer's replay at
/// once through `server`; returns nothing until they've all finished.
fn run_peers(ys: &YSweet, server: &str, dir: &Path, v: &Vector, chaos_seed: u64) {
    let (vector, seed) = files(dir, v);
    let init = muppet(&ys.server(), &["init", &seed, "--doc", &v.name]);
    assert!(
        init.status.success(),
        "init: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let wrong = if support::sabotage() {
        let other = xtxp_fuzzer::generate_single_round(9_999);
        let (path, _) = files(
            dir,
            &Vector {
                name: format!("{}-wrong", v.name),
                ..other
            },
        );
        Some(path)
    } else {
        None
    };
    let children: Vec<_> = v
        .peers
        .keys()
        .enumerate()
        .map(|(i, peer)| {
            let file = match (&wrong, i) {
                (Some(w), 0) => w.clone(),
                _ => vector.clone(),
            };
            let args: Vec<String> = [
                "replay",
                &file,
                "--peer",
                peer,
                "--doc",
                &v.name,
                "--seed",
                &(chaos_seed * 10 + i as u64).to_string(),
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            spawn_muppet(server, &args)
        })
        .collect();
    for (i, child) in children.into_iter().enumerate() {
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "peer {i} failed ({}): {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
    }
}

/// After all peers finish: a late joiner's view must match the in-process reference.
fn assert_converged(ys: &YSweet, v: &Vector, chaos_seed: u64) -> support::Hashes {
    let (doc_hash, snapshot_hash) = reference(v);
    let late = hashes(&muppet(&ys.server(), &["show", &v.name, "--hashes"]));
    if late.doc_hash != doc_hash || late.snapshot_hash != snapshot_hash {
        let canonical = String::from_utf8_lossy(
            &muppet(&ys.server(), &["show", &v.name, "--canonical"]).stdout,
        )
        .into_owned();
        panic!(
            "diverged (chaos seed {chaos_seed}):\nvector {}\nexpected doc {doc_hash} snapshot {snapshot_hash}\ngot      doc {} snapshot {}\ny-sweet's canonical XML: {canonical}",
            v.to_json(),
            late.doc_hash,
            late.snapshot_hash
        );
    }
    late
}

#[test]
fn three_peers_converge_through_chaos() {
    let bin = require_ysweet!();
    let dir = support::temp_dir("converge");
    let ys = YSweet::start(&bin, &dir);
    for chaos_seed in 0..5u64 {
        let v = xtxp_fuzzer::generate_single_round(100 + chaos_seed);
        let proxy = Proxy::start(
            ys.addr(),
            Chaos {
                seed: chaos_seed,
                delay_ms: 20,
                dup: 0.05,
                drop: 0.02,
            },
        );
        run_peers(&ys, &proxy.server(), &dir, &v, chaos_seed);
        assert_converged(&ys, &v, chaos_seed);
    }
}

#[test]
fn late_joiner_loads_in_one_update() {
    let bin = require_ysweet!();
    let dir = support::temp_dir("late");
    let ys = YSweet::start(&bin, &dir);
    let v = xtxp_fuzzer::generate_single_round(200);
    let proxy = Proxy::start(
        ys.addr(),
        Chaos {
            seed: 7,
            delay_ms: 10,
            dup: 0.0,
            drop: 0.0,
        },
    );
    run_peers(&ys, &proxy.server(), &dir, &v, 7);
    let late = assert_converged(&ys, &v, 7);
    assert_eq!(
        late.updates_before_synced, 1,
        "the whole document in one update"
    );
}

#[test]
fn offline_edits_survive_reconnects() {
    let bin = require_ysweet!();
    let dir = support::temp_dir("offline");
    let ys = YSweet::start(&bin, &dir);
    for chaos_seed in 0..3u64 {
        let v = xtxp_fuzzer::generate_single_round(300 + chaos_seed);
        let proxy = Proxy::start(
            ys.addr(),
            Chaos {
                seed: 50 + chaos_seed,
                delay_ms: 10,
                dup: 0.0,
                drop: 0.1,
            },
        );
        run_peers(&ys, &proxy.server(), &dir, &v, 50 + chaos_seed);
        assert_converged(&ys, &v, 50 + chaos_seed);
    }
}

#[test]
fn ysweet_restart_keeps_the_document() {
    let bin = require_ysweet!();
    let dir = support::temp_dir("restart");
    let mut ys = YSweet::start(&bin, &dir);
    let v = xtxp_fuzzer::generate_single_round(400);
    let (vector, seed) = files(&dir, &v);
    let server = ys.server();
    assert!(muppet(&server, &["init", &seed, "--doc", &v.name])
        .status
        .success());
    let peers: Vec<&String> = v.peers.keys().collect();
    let first = muppet(
        &server,
        &["replay", &vector, "--peer", peers[0], "--doc", &v.name],
    );
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    // Past one checkpoint, so y-sweet has written it to disk.
    std::thread::sleep(std::time::Duration::from_millis(2_500));
    ys.restart();
    for peer in &peers[1..] {
        let out = muppet(
            &ys.server(),
            &["replay", &vector, "--peer", peer, "--doc", &v.name],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_converged(&ys, &v, 0);
}

#[test]
fn init_refuses_existing_content() {
    let bin = require_ysweet!();
    let dir = support::temp_dir("init");
    let ys = YSweet::start(&bin, &dir);
    let fixture = support::repo_root().join("crates/xmesh/tests/fixtures/inkscape-blank.svg");
    let fixture = fixture.to_str().unwrap();
    let first = hashes(&muppet(&ys.server(), &["init", fixture, "--doc", "demo"]));
    let again = muppet(&ys.server(), &["init", fixture, "--doc", "demo"]);
    assert_eq!(again.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&again.stderr).contains("already has content"));
    let now = hashes(&muppet(&ys.server(), &["show", "demo", "--hashes"]));
    assert_eq!(now.doc_hash, first.doc_hash, "nothing changed");
}
