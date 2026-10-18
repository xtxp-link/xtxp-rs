//! Node IDs: the Yrs ID of the item that created a node (the spec's node ID).
//! They let a view follow a node while its path shifts under concurrent edits.

use crate::doc::{out_to_string, sort_attrs};
use crate::names::is_reserved;
use crate::Xmesh;
use yrs::branch::BranchID;
use yrs::types::xml::XmlOut;
use yrs::{GetString, ReadTxn, Transact, Xml, XmlFragment};

/// The Yrs ID of the item that created a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    pub client: u64,
    pub clock: u32,
}

/// A node of the live tree, with its ID.
#[derive(Debug, Clone, PartialEq)]
pub struct IdNode {
    pub id: NodeId,
    pub node: IdNodeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IdNodeKind {
    /// Attributes in canonical order, as `tree()` returns them.
    Element {
        name: String,
        attrs: Vec<(String, String)>,
        children: Vec<IdNode>,
    },
    Text(String),
}

impl Xmesh {
    /// The live document like [`Xmesh::tree`], with each node's ID.
    pub fn tree_with_ids(&self) -> Vec<IdNode> {
        let fragment = self.fragment();
        let txn = self.doc.transact();
        read(&txn, &fragment)
    }

    /// The node's current path from the root element, as [`crate::Op`] uses;
    /// `None` if it's deleted or outside the root element.
    pub fn path_of(&self, id: NodeId) -> Option<Vec<u32>> {
        let tree = self.tree_with_ids();
        let root = tree.iter().find(|n| match &n.node {
            IdNodeKind::Element { name, .. } => !is_reserved(name),
            IdNodeKind::Text(_) => false,
        })?;
        let mut path = Vec::new();
        find(root, id, &mut path).then_some(path)
    }
}

fn find(node: &IdNode, id: NodeId, path: &mut Vec<u32>) -> bool {
    if node.id == id {
        return true;
    }
    if let IdNodeKind::Element { children, .. } = &node.node {
        for (i, child) in children.iter().enumerate() {
            path.push(i as u32);
            if find(child, id, path) {
                return true;
            }
            path.pop();
        }
    }
    false
}

fn node_id(out: &XmlOut) -> Option<NodeId> {
    match out.id() {
        BranchID::Nested(id) => Some(NodeId {
            client: id.client.get(),
            clock: id.clock,
        }),
        BranchID::Root(_) => None,
    }
}

fn read<F: XmlFragment, T: ReadTxn>(txn: &T, parent: &F) -> Vec<IdNode> {
    parent
        .children(txn)
        .filter_map(|child| {
            let id = node_id(&child)?;
            let node = match &child {
                XmlOut::Text(t) => IdNodeKind::Text(t.get_string(txn)),
                XmlOut::Element(e) => {
                    let mut attrs: Vec<(String, String)> = e
                        .attributes(txn)
                        .map(|(k, v)| (k.to_string(), out_to_string(v)))
                        .collect();
                    sort_attrs(&mut attrs);
                    IdNodeKind::Element {
                        name: e.tag().to_string(),
                        attrs,
                        children: read(txn, e),
                    }
                }
                XmlOut::Fragment(_) => return None,
            };
            Some(IdNode { id, node })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, Op};

    const SVG: &str =
        r#"<!--c--><svg xmlns="http://www.w3.org/2000/svg"><rect id="a"/>t<g><circle/></g></svg>"#;

    fn doc(client: u64) -> Xmesh {
        let d = Xmesh::new(client).unwrap();
        d.apply(&Xmesh::seed(SVG).unwrap()).unwrap();
        d
    }

    fn strip(nodes: &[IdNode]) -> Vec<Node> {
        nodes
            .iter()
            .map(|n| match &n.node {
                IdNodeKind::Text(t) => Node::Text(t.clone()),
                IdNodeKind::Element {
                    name,
                    attrs,
                    children,
                } => Node::Element(crate::Element {
                    name: name.clone(),
                    attrs: attrs.clone(),
                    children: strip(children),
                }),
            })
            .collect()
    }

    fn all_ids(nodes: &[IdNode], out: &mut Vec<NodeId>) {
        for n in nodes {
            out.push(n.id);
            if let IdNodeKind::Element { children, .. } = &n.node {
                all_ids(children, out);
            }
        }
    }

    fn root_child(d: &Xmesh, i: usize) -> NodeId {
        let tree = d.tree_with_ids();
        let root = tree
            .iter()
            .find(|n| matches!(&n.node, IdNodeKind::Element { name, .. } if name.ends_with("}svg")))
            .unwrap();
        match &root.node {
            IdNodeKind::Element { children, .. } => children[i].id,
            _ => unreachable!(),
        }
    }

    #[test]
    fn ids_are_stable_across_replicas() {
        assert_eq!(doc(1).tree_with_ids(), doc(2).tree_with_ids());
    }

    #[test]
    fn tree_with_ids_matches_tree() {
        let d = doc(1);
        assert_eq!(strip(&d.tree_with_ids()), d.tree());
    }

    #[test]
    fn path_follows_node_after_insert_above() {
        let d = doc(1);
        let rect = root_child(&d, 0);
        assert_eq!(d.path_of(rect), Some(vec![0]));
        d.edit(&[Op::Insert {
            parent: vec![],
            index: 0,
            xml: "<ellipse/>".into(),
        }])
        .unwrap();
        assert_eq!(d.path_of(rect), Some(vec![1]));
    }

    #[test]
    fn path_of_deleted_is_none() {
        let d = doc(1);
        let circle_parent = root_child(&d, 2);
        d.edit(&[Op::Remove { path: vec![2] }]).unwrap();
        assert_eq!(d.path_of(circle_parent), None);
    }

    #[test]
    fn path_of_root_is_empty_and_outside_root_is_none() {
        let d = doc(1);
        let tree = d.tree_with_ids();
        assert_eq!(d.path_of(tree[1].id), Some(vec![]), "the svg root");
        assert_eq!(
            d.path_of(tree[0].id),
            None,
            "the prolog comment is outside the root"
        );
    }

    #[test]
    fn ids_differ_per_node() {
        let mut ids = Vec::new();
        all_ids(&doc(1).tree_with_ids(), &mut ids);
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
        assert!(n >= 6);
    }
}
