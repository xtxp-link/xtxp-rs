//! A step 2 from y-sweet v0.9.1 (Yrs 0.19.2) that lists one client's blocks
//! twice, captured from the e2e test `offline_edits_survive_reconnects`.
//! Applied raw over a document holding an integrated skip for that client,
//! Yrs 0.28 frees a live block and the next read crashes. Its own process, so
//! a crash fails only this test.

use xmesh::Xmesh;

const BASE: &[u8] = include_bytes!("fixtures/repeat-base.bin");
const STEP2: &[u8] = include_bytes!("fixtures/repeat-step2.bin");

#[test]
fn repeated_blocks_apply_safely() {
    let doc = Xmesh::new(0x4321).unwrap();
    doc.apply(BASE).unwrap();
    doc.apply(STEP2).unwrap();
    let canonical = doc.canonical();
    assert!(
        canonical.contains(r#"<ellipse fill="é😀" id="e1"#),
        "{canonical}"
    );
    assert!(!doc.has_missing_updates());
    // Reading again, and applying the repeat again, changes nothing.
    doc.apply(STEP2).unwrap();
    assert_eq!(doc.canonical(), canonical);
    assert_eq!(doc.tree_with_ids().len(), doc.tree().len());
}
