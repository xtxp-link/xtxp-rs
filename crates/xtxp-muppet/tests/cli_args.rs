//! Argument parsing for each subcommand, and a clean failure with no y-sweet.

use clap::Parser;
use xtxp_muppet::adapters::cli::{parse_edit_ops, Cli, Sub};

#[test]
fn parses_each_subcommand() {
    let c = Cli::try_parse_from(["xtxp-muppet", "init", "a.svg", "--doc", "demo"]).unwrap();
    assert!(matches!(c.sub, Sub::Init { ref doc, .. } if doc.as_deref() == Some("demo")));
    assert_eq!(c.server, "localhost:8080");
    let c = Cli::try_parse_from([
        "xtxp-muppet",
        "edit",
        "demo",
        "set-attr",
        "0",
        "fill",
        "red",
        ",",
        "remove",
        "1",
    ])
    .unwrap();
    match c.sub {
        Sub::Edit { ops, .. } => assert_eq!(parse_edit_ops(&ops).unwrap().len(), 2),
        other => panic!("{other:?}"),
    }
    let c = Cli::try_parse_from(["xtxp-muppet", "show", "demo", "--hashes"]).unwrap();
    assert!(matches!(c.sub, Sub::Show { hashes: true, .. }));
    let c = Cli::try_parse_from([
        "xtxp-muppet",
        "replay",
        "v.json",
        "--peer",
        "p1",
        "--seed",
        "3",
    ])
    .unwrap();
    assert!(matches!(c.sub, Sub::Replay { seed: 3, .. }));
}

#[test]
fn edit_rejects_bad_op_words() {
    let c = Cli::try_parse_from(["xtxp-muppet", "edit", "demo", "frobnicate", "0"]).unwrap();
    match c.sub {
        Sub::Edit { ops, .. } => assert!(parse_edit_ops(&ops).is_err()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn one_shot_without_server_fails_cleanly() {
    let start = std::time::Instant::now();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_xtxp-muppet"))
        .args(["--server", "127.0.0.1:1", "show", "demo"])
        .output()
        .unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(15));
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("could not reach y-sweet"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn bad_doc_names_are_refused_at_once() {
    for args in [
        &["show", "my doc"][..],
        &["edit", "a?b", "remove", "0"],
        &["replay", "v.json", "--peer", "p1", "--doc", "x y"],
    ] {
        let start = std::time::Instant::now();
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_xtxp-muppet"))
            .arg("--server")
            .arg("127.0.0.1:1")
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "{args:?}"
        );
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(
            stderr.contains("isn't a valid document name"),
            "{args:?}: {stderr}"
        );
    }
}
