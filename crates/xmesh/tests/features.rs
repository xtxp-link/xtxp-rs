//! Feature files and vectors must agree: every `@vector:<name>` tag names a
//! vector file, and every hand-written vector is named by some scenario.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files(dir: &str, ext: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(root().join(dir))
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == ext))
        .collect();
    out.sort();
    out
}

fn tagged() -> BTreeSet<String> {
    files("features", "feature")
        .iter()
        .flat_map(|p| {
            std::fs::read_to_string(p)
                .unwrap()
                .split_whitespace()
                .filter_map(|w| w.strip_prefix("@vector:").map(str::to_string))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn vectors() -> BTreeSet<String> {
    files("vectors", "json")
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn every_tag_has_a_vector() {
    let missing: Vec<_> = tagged().difference(&vectors()).cloned().collect();
    assert!(missing.is_empty(), "tags with no vector file: {missing:?}");
}

#[test]
fn every_vector_is_tagged() {
    let untagged: Vec<_> = vectors().difference(&tagged()).cloned().collect();
    assert!(untagged.is_empty(), "vectors no scenario names: {untagged:?}");
}
