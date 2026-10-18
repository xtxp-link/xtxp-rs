//! Runs every hand-written vector in `vectors/` through the convergence check.

use std::path::Path;
use xmesh::vector::{check, Vector};

#[test]
fn all_vectors_pass() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vectors");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no vectors in {}", dir.display());
    let mut failures = Vec::new();
    for path in &paths {
        let json = std::fs::read_to_string(path).unwrap();
        let result = Vector::from_json(&json).and_then(|v| check(&v, 200, 0));
        if let Err(e) = result {
            failures.push(format!("{}: {e}", path.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} vectors failed:\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n")
    );
}
