//! What changed between two trees, as patch sets.

use super::html::{marker_name, render_node, render_region, TransitionNames};
use std::collections::BTreeMap;
use xmesh::{IdNode, IdNodeKind, NodeId};

/// How much one patch replaces: each changed node, or the whole drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grain {
    Node,
    Region,
}

/// One change, rendered at both grains.
#[derive(Clone, Debug, PartialEq)]
pub struct PatchSet {
    pub seq: u64,
    pub sent_ms: u64,
    pub snapshot: String,
    pub node_html: String,
    pub region_html: String,
}

impl PatchSet {
    pub fn html(&self, grain: Grain) -> &str {
        match grain {
            Grain::Node => &self.node_html,
            Grain::Region => &self.region_html,
        }
    }
}

/// A declarative-partial-update patch: `<template for>` around `inner`.
pub fn template(name: &str, seq: u64, sent_ms: u64, inner: &str) -> String {
    format!(
        "<template for=\"{name}\" data-seq=\"{seq}\" data-sent=\"{sent_ms}\">{inner}</template>"
    )
}

/// Turns successive trees into patch sets.
#[derive(Default)]
pub struct Renderer {
    names: TransitionNames,
    tree: Vec<IdNode>,
    region: String,
    snapshot: String,
    seq: u64,
    records: BTreeMap<NodeId, String>,
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer::default()
    }

    /// The patch set from the last tree to `tree`, or `None` if nothing changed.
    pub fn update(&mut self, tree: &[IdNode], snapshot: &str, now_ms: u64) -> Option<PatchSet> {
        let root = root_of(tree);
        let mut records = BTreeMap::new();
        let mut parents = BTreeMap::new();
        if let Some(root) = root {
            record(root, None, &mut records, &mut parents);
        }
        let dirty: Vec<NodeId> = records
            .iter()
            .filter(|(id, rec)| self.records.get(*id) != Some(*rec))
            .map(|(id, _)| *id)
            .collect();
        let root_changed = root.map(|r| r.id) != root_of(&self.tree).map(|r| r.id);
        if dirty.is_empty() && !root_changed {
            return None;
        }
        self.seq += 1;
        let seq = self.seq;
        self.region = render_region(tree, &mut self.names);
        let region_html = template("doc", seq, now_ms, &self.region);
        let whole = root_changed || root.is_some_and(|r| dirty.contains(&r.id));
        let node_html = if whole {
            region_html.clone()
        } else {
            let tops: Vec<NodeId> = dirty
                .iter()
                .filter(|id| {
                    let mut up = parents.get(*id).copied().flatten();
                    while let Some(p) = up {
                        if dirty.contains(&p) {
                            return false;
                        }
                        up = parents.get(&p).copied().flatten();
                    }
                    true
                })
                .copied()
                .collect();
            let mut html = String::new();
            for n in in_order(root.expect("not whole, so there is a root"), &tops) {
                let inner = render_node(n, &mut self.names);
                html.push_str(&template(&marker_name(n.id), seq, now_ms, &inner));
            }
            html
        };
        self.records = records;
        self.tree = tree.to_vec();
        self.snapshot = snapshot.to_string();
        Some(PatchSet {
            seq,
            sent_ms: now_ms,
            snapshot: snapshot.to_string(),
            node_html,
            region_html,
        })
    }

    /// The whole drawing as one region patch, at the current sequence number.
    pub fn full(&self, now_ms: u64) -> PatchSet {
        let html = template("doc", self.seq, now_ms, &self.region);
        PatchSet {
            seq: self.seq,
            sent_ms: now_ms,
            snapshot: self.snapshot.clone(),
            node_html: html.clone(),
            region_html: html,
        }
    }

    /// Takes the next sequence number for a patch set made outside the tree,
    /// such as the connection state.
    pub fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    pub fn tree(&self) -> &[IdNode] {
        &self.tree
    }

    /// The current region's HTML, without a template, for rendering pages.
    pub fn region(&self) -> &str {
        &self.region
    }
}

/// The document's root element: its first element that isn't a comment or the like.
fn root_of(tree: &[IdNode]) -> Option<&IdNode> {
    tree.iter().find(
        |n| matches!(&n.node, IdNodeKind::Element { name, .. } if !xmesh::names::is_reserved(name)),
    )
}

/// A node's own content and its children's IDs, so any change to it, or to
/// which children it has, shows up as a different record.
fn record(
    n: &IdNode,
    parent: Option<NodeId>,
    records: &mut BTreeMap<NodeId, String>,
    parents: &mut BTreeMap<NodeId, Option<NodeId>>,
) {
    parents.insert(n.id, parent);
    match &n.node {
        IdNodeKind::Text(t) => {
            records.insert(n.id, format!("t{t}"));
        }
        IdNodeKind::Element {
            name,
            attrs,
            children,
        } => {
            let ids: Vec<NodeId> = children.iter().map(|c| c.id).collect();
            records.insert(n.id, format!("e{name}{attrs:?}{ids:?}"));
            for c in children {
                record(c, Some(n.id), records, parents);
            }
        }
    }
}

/// The nodes in `wanted`, in document order.
fn in_order<'a>(n: &'a IdNode, wanted: &[NodeId]) -> Vec<&'a IdNode> {
    let mut out = Vec::new();
    if wanted.contains(&n.id) {
        out.push(n);
    } else if let IdNodeKind::Element { children, .. } = &n.node {
        for c in children {
            out.extend(in_order(c, wanted));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use xmesh::{Op, Xmesh};

    const LOGO: &str = include_str!("../../../xmesh/tests/fixtures/xtxp-logo.svg");

    fn logo() -> Xmesh {
        let d = Xmesh::new(7).unwrap();
        d.apply(&Xmesh::seed(LOGO).unwrap()).unwrap();
        d
    }

    fn update(r: &mut Renderer, d: &Xmesh) -> Option<PatchSet> {
        r.update(&d.tree_with_ids(), &d.snapshot_hash(), 1000)
    }

    fn fors(html: &str) -> Vec<String> {
        html.split("<template for=\"")
            .skip(1)
            .map(|s| s[..s.find('"').unwrap()].to_string())
            .collect()
    }

    fn id_at(d: &Xmesh, path: &[u32]) -> String {
        let tree = d.tree_with_ids();
        let mut node = tree.iter().find(|n| matches!(&n.node, IdNodeKind::Element { name, .. } if !xmesh::names::is_reserved(name))).unwrap();
        for i in path {
            let IdNodeKind::Element { children, .. } = &node.node else {
                panic!()
            };
            node = &children[*i as usize];
        }
        marker_name(node.id)
    }

    fn set(path: Vec<u32>, name: &str, value: &str) -> Op {
        Op::SetAttr {
            path,
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn template_wraps_markers() {
        assert_eq!(
            template("n1-2", 3, 4, "<?start name=\"n1-2\">x<?end>"),
            "<template for=\"n1-2\" data-seq=\"3\" data-sent=\"4\"><?start name=\"n1-2\">x<?end></template>"
        );
    }

    #[test]
    fn first_update_is_one_region_patch() {
        let mut r = Renderer::new();
        let p = update(&mut r, &logo()).unwrap();
        assert_eq!(p.seq, 1);
        assert_eq!(fors(&p.node_html), vec!["doc"]);
        assert_eq!(fors(&p.region_html), vec!["doc"]);
        assert!(p.region_html.contains("stroke-width=\"9.9\""));
        assert!(r.region().starts_with("<?start name=\"doc\"><svg"));
    }

    #[test]
    fn attribute_change_patches_only_that_element() {
        let d = logo();
        let mut r = Renderer::new();
        update(&mut r, &d);
        d.edit(&[set(vec![2, 0], "stroke-width", "14")]).unwrap();
        let p = update(&mut r, &d).unwrap();
        assert_eq!(p.seq, 2);
        assert_eq!(fors(&p.node_html), vec![id_at(&d, &[2, 0])]);
        assert!(p.node_html.contains("stroke-width=\"14\""));
        assert_eq!(fors(&p.region_html), vec!["doc"]);
        assert!(r.region().contains("stroke-width=\"14\""));
    }

    #[test]
    fn text_change_patches_only_the_text_node() {
        let d = logo();
        let mut r = Renderer::new();
        update(&mut r, &d);
        d.edit(&[Op::InsertText {
            path: vec![0, 0],
            offset: 0,
            text: "/*x*/".into(),
        }])
        .unwrap();
        let p = update(&mut r, &d).unwrap();
        assert_eq!(fors(&p.node_html), vec![id_at(&d, &[0, 0])]);
    }

    #[test]
    fn insert_and_delete_patch_the_parent() {
        let d = logo();
        let mut r = Renderer::new();
        update(&mut r, &d);
        let g = id_at(&d, &[2]);
        d.edit(&[Op::Insert {
            parent: vec![2],
            index: 0,
            xml: "<circle r=\"3\"/>".into(),
        }])
        .unwrap();
        let p = update(&mut r, &d).unwrap();
        assert_eq!(fors(&p.node_html), vec![g.clone()]);
        assert!(p.node_html.contains("<circle r=\"3\""));
        d.edit(&[Op::Remove { path: vec![2, 0] }]).unwrap();
        let p = update(&mut r, &d).unwrap();
        assert_eq!(fors(&p.node_html), vec![g]);
        assert!(!p.node_html.contains("<circle"));
    }

    #[test]
    fn move_patches_both_parents_once() {
        let d = logo();
        let mut r = Renderer::new();
        update(&mut r, &d);
        let (left, right) = (id_at(&d, &[2, 1]), id_at(&d, &[2, 2]));
        d.edit(&[Op::Move {
            path: vec![2, 1, 0],
            new_parent: vec![2, 2],
            index: 0,
        }])
        .unwrap();
        let mut got = fors(&update(&mut r, &d).unwrap().node_html);
        got.sort();
        let mut want = vec![left, right];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn no_patch_for_children_of_a_dirty_node() {
        let d = logo();
        let mut r = Renderer::new();
        update(&mut r, &d);
        d.edit(&[
            set(vec![2], "opacity", "0.5"),
            set(vec![2, 0], "stroke-width", "14"),
        ])
        .unwrap();
        assert_eq!(
            fors(&update(&mut r, &d).unwrap().node_html),
            vec![id_at(&d, &[2])]
        );
    }

    #[test]
    fn root_change_is_a_region_patch() {
        let d = logo();
        let mut r = Renderer::new();
        update(&mut r, &d);
        d.edit(&[set(vec![], "opacity", "0.5")]).unwrap();
        assert_eq!(fors(&update(&mut r, &d).unwrap().node_html), vec!["doc"]);
    }

    #[test]
    fn same_tree_after_reconnect_gives_no_patch() {
        let d = logo();
        let mut r = Renderer::new();
        update(&mut r, &d);
        let copy = Xmesh::new(99).unwrap();
        copy.apply(&d.diff(&Xmesh::new(98).unwrap().state_vector()).unwrap())
            .unwrap();
        assert_eq!(update(&mut r, &copy), None);
    }

    #[test]
    fn empty_tree_renders_empty_region() {
        let d = logo();
        let mut r = Renderer::new();
        update(&mut r, &d);
        let p = r.update(&[], "e", 1000).unwrap();
        assert_eq!(p.region_html, "<template for=\"doc\" data-seq=\"2\" data-sent=\"1000\"><?start name=\"doc\"><?end></template>");
        assert_eq!(p.node_html, p.region_html);
        let p = update(&mut r, &d).unwrap();
        assert!(p.region_html.contains("<svg"));
    }

    #[test]
    fn full_is_a_region_patch_at_current_seq() {
        let d = logo();
        let mut r = Renderer::new();
        update(&mut r, &d);
        d.edit(&[set(vec![1], "fill", "#000")]).unwrap();
        update(&mut r, &d);
        let f = r.full(5000);
        assert_eq!(f.seq, 2);
        assert_eq!(f.sent_ms, 5000);
        assert_eq!(fors(&f.region_html), vec!["doc"]);
        assert!(f.region_html.contains("fill=\"#000\""));
        assert_eq!(f.snapshot, d.snapshot_hash());
    }
}
