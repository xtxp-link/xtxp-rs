//! Every vector's updates, made by Xmesh on Yrs 0.28, applied to a Yrs 0.19.2
//! document (what y-sweet v0.9.1 holds), must give the same canonical XML.

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use std::path::Path;
use xmesh::vector::{replay, Vector};
use xmesh::{canonical, Xmesh};
use yrs019::updates::decoder::Decode;
use yrs019::{Doc, OffsetKind, Options, Transact, Update};
use yrs_compat::tree_019;

fn doc_019() -> Doc {
    Doc::with_options(Options {
        skip_gc: true,
        offset_kind: OffsetKind::Utf16,
        ..Options::with_client_id(0xF000_0001)
    })
}

fn vectors() -> Vec<Vector> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vectors");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .map(|p| Vector::from_json(&std::fs::read_to_string(p).unwrap()).unwrap())
        .collect()
}

#[test]
fn every_vector_matches_on_019() {
    let mut mismatches = Vec::new();
    for v in vectors() {
        let r = replay(&v).unwrap();
        let mut all = vec![r.seed.clone()];
        all.extend(r.updates.iter().cloned());

        let new = Xmesh::new(0xF000_0000).unwrap();
        let old = doc_019();
        for u in &all {
            new.apply(u).unwrap();
            let update = Update::decode_v1(u).expect("0.19 decodes a 0.28 update");
            old.transact_mut().apply_update(update);
        }
        let old_canonical = canonical(&tree_019(&old));
        if old_canonical != new.canonical() {
            mismatches.push(format!(
                "{}:\n  0.28 {}\n  0.19 {old_canonical}",
                v.name,
                new.canonical()
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn reads_reserved_and_text_nodes() {
    let xml = r#"<!--p--><svg xmlns="http://www.w3.org/2000/svg"><?t d?>hi<g a="1"/></svg>"#;
    let old = doc_019();
    old.transact_mut()
        .apply_update(Update::decode_v1(&Xmesh::seed(xml).unwrap()).unwrap());
    assert_eq!(
        canonical(&tree_019(&old)),
        canonical(&xmesh::parse_document(xml).unwrap())
    );
}

/// What y-sweet sees behind the chaos proxy: updates out of order. Yrs 0.27.1
/// and 0.27.2 fixed pending-update and delete-set bugs that 0.19.2 still has.
#[test]
#[ignore = "finding: Yrs 0.19.2 loses deletes that arrive before their inserts (fixed in 0.27.2); 6 of 14 vectors diverge; run with --ignored to see them"]
fn shuffled_orders_match_on_019() {
    let mut rng = StdRng::seed_from_u64(0);
    let mut mismatches = Vec::new();
    for v in vectors() {
        let r = replay(&v).unwrap();
        let mut all = vec![r.seed.clone()];
        all.extend(r.updates.iter().cloned());
        let reference = Xmesh::new(0xF000_0000).unwrap();
        for u in &all {
            reference.apply(u).unwrap();
        }
        for order in 0..200 {
            let mut shuffled = all.clone();
            shuffled.shuffle(&mut rng);
            let old = doc_019();
            for u in &shuffled {
                old.transact_mut()
                    .apply_update(Update::decode_v1(u).unwrap());
            }
            let got = canonical(&tree_019(&old));
            if got != reference.canonical() {
                mismatches.push(format!(
                    "{} order {order}:\n  0.28 {}\n  0.19 {got}",
                    v.name,
                    reference.canonical()
                ));
                break;
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} vectors diverge on 0.19.2:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

/// Smallest case of the finding: a delete delivered before the insert it
/// deletes is lost by Yrs 0.19.2 and kept by 0.28.
#[test]
fn finding_019_loses_a_delete_that_arrives_first() {
    let seed =
        Xmesh::seed(r#"<svg xmlns="http://www.w3.org/2000/svg"><rect/><circle/></svg>"#).unwrap();
    let editor = Xmesh::new(1).unwrap();
    editor.apply(&seed).unwrap();
    let delete = editor.edit(&[xmesh::Op::Remove { path: vec![1] }]).unwrap();

    let new = Xmesh::new(2).unwrap();
    new.apply(&delete).unwrap();
    new.apply(&seed).unwrap();
    assert!(!new.canonical().contains("circle"), "0.28 keeps the delete");

    let old = doc_019();
    old.transact_mut()
        .apply_update(Update::decode_v1(&delete).unwrap());
    old.transact_mut()
        .apply_update(Update::decode_v1(&seed).unwrap());
    assert!(
        canonical(&tree_019(&old)).contains("circle"),
        "0.19.2 no longer loses the delete: the finding is gone, update the spec"
    );

    let in_order = doc_019();
    in_order
        .transact_mut()
        .apply_update(Update::decode_v1(&seed).unwrap());
    in_order
        .transact_mut()
        .apply_update(Update::decode_v1(&delete).unwrap());
    assert!(
        !canonical(&tree_019(&in_order)).contains("circle"),
        "in order, 0.19.2 is fine"
    );
}

/// Finding: Yrs 0.19.2's v1 decoder reads client IDs as `u32`, so a client
/// ID of 2^32 or more (allowed by Yrs 0.28) comes out as a different client
/// in y-sweet. Xmesh therefore only accepts 32-bit client IDs, as Yjs uses.
#[test]
fn finding_019_reads_client_ids_as_32_bits() {
    use yrs019::ReadTxn;
    let big = (1u64 << 40) + 5;
    let new = yrs::Doc::with_options(yrs::Options::with_client_id(yrs::block::ClientID::new(big)));
    let text = new.get_or_insert_text("t");
    {
        use yrs::{Text, Transact};
        text.insert(&mut new.transact_mut(), 0, "x");
    }
    let update = {
        use yrs::{ReadTxn, Transact};
        new.transact()
            .encode_state_as_update_v1(&yrs::StateVector::default())
    };
    let old = doc_019();
    old.transact_mut()
        .apply_update(Update::decode_v1(&update).unwrap());
    let clients: Vec<u64> = old
        .transact()
        .state_vector()
        .iter()
        .map(|(c, _)| *c)
        .collect();
    assert_eq!(clients.len(), 1);
    assert_ne!(
        clients[0], big,
        "0.19.2 no longer truncates: the finding is gone, update the spec"
    );
    assert!(
        xmesh::Xmesh::new(big).is_err(),
        "Xmesh refuses IDs that y-sweet would mangle"
    );
}
