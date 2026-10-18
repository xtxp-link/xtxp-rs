//! Edit operations. One call to [`Xmesh::edit`] is one Yrs transaction.

use crate::doc::{new_doc, read_node, write_nodes, FRAGMENT};
use crate::names::{expand, CDATA, COMMENT, FIXED_PREFIXES, PI};
use crate::node::{parse_fragment, Node};
use crate::Error;
use crate::Xmesh;
use serde::{Deserialize, Serialize};
use yrs::types::xml::XmlOut;
use yrs::updates::decoder::Decode;
use yrs::{
    Doc, ReadTxn, StateVector, Text, Transact, TransactionMut, Update, Xml, XmlFragment,
    XmlFragmentRef,
};

/// One edit. Paths are child indexes among live children, starting at the root
/// element (`[]` is the root element). Names are expanded (`{uri}local`), bare
/// (`fill`), or use a fixed prefix (`inkscape:label`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Insert {
        parent: Vec<u32>,
        index: u32,
        xml: String,
    },
    Remove {
        path: Vec<u32>,
    },
    SetAttr {
        path: Vec<u32>,
        name: String,
        value: String,
    },
    RemoveAttr {
        path: Vec<u32>,
        name: String,
    },
    InsertText {
        path: Vec<u32>,
        offset: u32,
        text: String,
    },
    DeleteText {
        path: Vec<u32>,
        offset: u32,
        len: u32,
    },
    /// `index` counts `new_parent`'s children before the original is removed.
    Move {
        path: Vec<u32>,
        new_parent: Vec<u32>,
        index: u32,
    },
}

impl Xmesh {
    /// Applies `ops` as one transaction and returns the update it produced.
    /// If any operation fails, the document is left unchanged.
    pub fn edit(&self, ops: &[Op]) -> Result<Vec<u8>, Error> {
        // Yrs can't roll a transaction back, so the batch is tried on a
        // throwaway copy first and only replayed here if every op succeeds.
        let scratch = new_doc(self.client_id());
        let full = self
            .doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let update = Update::decode_v1(&full).map_err(|e| Error::Update(e.to_string()))?;
        scratch
            .transact_mut()
            .apply_update(update)
            .map_err(|e| Error::Update(e.to_string()))?;
        run(&scratch, ops)?;

        let before = self.doc.transact().state_vector();
        run(&self.doc, ops)?;
        Ok(self.doc.transact().encode_diff_v1(&before))
    }
}

fn run(doc: &Doc, ops: &[Op]) -> Result<(), Error> {
    let fragment = doc.get_or_insert_xml_fragment(FRAGMENT);
    let mut txn = doc.transact_mut();
    for op in ops {
        apply_op(&mut txn, &fragment, op)?;
    }
    Ok(())
}

/// A node that can hold children.
enum Parent {
    Fragment(XmlFragmentRef),
    Element(yrs::XmlElementRef),
}

impl Parent {
    fn from_out(out: XmlOut, path: &[u32]) -> Result<Parent, Error> {
        match out {
            XmlOut::Element(e) => Ok(Parent::Element(e)),
            XmlOut::Fragment(f) => Ok(Parent::Fragment(f)),
            XmlOut::Text(_) => Err(Error::NotAnElement(path.to_vec())),
        }
    }

    fn len<T: ReadTxn>(&self, txn: &T) -> u32 {
        match self {
            Parent::Fragment(f) => f.len(txn),
            Parent::Element(e) => e.len(txn),
        }
    }

    fn get<T: ReadTxn>(&self, txn: &T, index: u32) -> Option<XmlOut> {
        match self {
            Parent::Fragment(f) => f.get(txn, index),
            Parent::Element(e) => e.get(txn, index),
        }
    }

    fn insert(&self, txn: &mut TransactionMut, index: u32, nodes: &[Node]) {
        match self {
            Parent::Fragment(f) => write_nodes(txn, f, index, nodes),
            Parent::Element(e) => write_nodes(txn, e, index, nodes),
        }
    }

    fn remove(&self, txn: &mut TransactionMut, index: u32) {
        match self {
            Parent::Fragment(f) => f.remove(txn, index),
            Parent::Element(e) => e.remove(txn, index),
        }
    }
}

/// Index of the root element among the fragment's live children.
fn root_index<T: ReadTxn>(txn: &T, fragment: &XmlFragmentRef) -> Result<u32, Error> {
    fragment
        .children(txn)
        .position(|c| matches!(&c, XmlOut::Element(e) if ![COMMENT, PI, CDATA].contains(&e.tag().as_ref())))
        .map(|i| i as u32)
        .ok_or_else(|| Error::BadPath(vec![]))
}

/// Resolves a path to its node, and to its parent and index unless it's the root.
fn resolve<T: ReadTxn>(
    txn: &T,
    fragment: &XmlFragmentRef,
    path: &[u32],
) -> Result<(XmlOut, Option<(Parent, u32)>), Error> {
    let root_at = root_index(txn, fragment)?;
    let mut node = fragment
        .get(txn, root_at)
        .ok_or_else(|| Error::BadPath(vec![]))?;
    let mut parent = None;
    for (depth, &i) in path.iter().enumerate() {
        let p =
            Parent::from_out(node, &path[..depth]).map_err(|_| Error::BadPath(path.to_vec()))?;
        node = p.get(txn, i).ok_or_else(|| Error::BadPath(path.to_vec()))?;
        parent = Some((p, i));
    }
    Ok((node, parent))
}

fn element_at<T: ReadTxn>(
    txn: &T,
    fragment: &XmlFragmentRef,
    path: &[u32],
) -> Result<yrs::XmlElementRef, Error> {
    match resolve(txn, fragment, path)?.0 {
        XmlOut::Element(e) => Ok(e),
        _ => Err(Error::NotAnElement(path.to_vec())),
    }
}

fn text_at<T: ReadTxn>(
    txn: &T,
    fragment: &XmlFragmentRef,
    path: &[u32],
) -> Result<yrs::XmlTextRef, Error> {
    match resolve(txn, fragment, path)?.0 {
        XmlOut::Text(t) => Ok(t),
        _ => Err(Error::NotText(path.to_vec())),
    }
}

/// Turns `fill`, `inkscape:label` or `{uri}local` into an expanded name.
fn expanded_name(name: &str) -> Result<String, Error> {
    if name.starts_with('{') {
        return Ok(name.to_string());
    }
    match name.split_once(':') {
        None => Ok(name.to_string()),
        Some((prefix, local)) => FIXED_PREFIXES
            .iter()
            .find(|(p, _)| *p == prefix)
            .map(|(_, uri)| expand(Some(uri), local))
            .ok_or_else(|| Error::Xml(format!("unknown prefix {prefix:?} in {name:?}"))),
    }
}

fn apply_op(txn: &mut TransactionMut, fragment: &XmlFragmentRef, op: &Op) -> Result<(), Error> {
    match op {
        Op::Insert { parent, index, xml } => {
            let nodes = parse_fragment(xml)?;
            let p = Parent::from_out(resolve(txn, fragment, parent)?.0, parent)?;
            if *index > p.len(txn) {
                return Err(Error::IndexOutOfRange(parent.clone(), *index));
            }
            p.insert(txn, *index, &nodes);
        }
        Op::Remove { path } => {
            let (_, parent) = resolve(txn, fragment, path)?;
            let (p, i) = parent.ok_or_else(|| Error::BadPath(path.clone()))?;
            p.remove(txn, i);
        }
        Op::SetAttr { path, name, value } => {
            let name = expanded_name(name)?;
            element_at(txn, fragment, path)?.insert_attribute(txn, name, value.clone());
        }
        Op::RemoveAttr { path, name } => {
            let name = expanded_name(name)?;
            element_at(txn, fragment, path)?.remove_attribute(txn, &name);
        }
        Op::InsertText { path, offset, text } => {
            let t = text_at(txn, fragment, path)?;
            if *offset > t.len(txn) {
                return Err(Error::IndexOutOfRange(path.clone(), *offset));
            }
            t.insert(txn, *offset, text);
        }
        Op::DeleteText { path, offset, len } => {
            let t = text_at(txn, fragment, path)?;
            if offset + len > t.len(txn) {
                return Err(Error::IndexOutOfRange(path.clone(), offset + len));
            }
            t.remove_range(txn, *offset, *len);
        }
        Op::Move {
            path,
            new_parent,
            index,
        } => {
            if path.is_empty() {
                return Err(Error::BadPath(path.clone()));
            }
            if new_parent.starts_with(path) {
                return Err(Error::MoveIntoSelf);
            }
            let (node, parent) = resolve(txn, fragment, path)?;
            let (old_parent, old_index) = parent.ok_or_else(|| Error::BadPath(path.clone()))?;
            let copy = read_node(txn, &node).ok_or_else(|| Error::BadPath(path.clone()))?;
            let dest = Parent::from_out(resolve(txn, fragment, new_parent)?.0, new_parent)?;
            if *index > dest.len(txn) {
                return Err(Error::IndexOutOfRange(new_parent.clone(), *index));
            }
            dest.insert(txn, *index, std::slice::from_ref(&copy));
            let same_parent = new_parent.as_slice() == &path[..path.len() - 1];
            let old_index = if same_parent && *index <= old_index {
                old_index + 1
            } else {
                old_index
            };
            old_parent.remove(txn, old_index);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg"><g id="g"><rect id="r"/></g><text>é😀z</text><circle id="c"/></svg>"#;
    const NS: &str = r#" xmlns="http://www.w3.org/2000/svg""#;

    fn doc() -> Xmesh {
        let doc = Xmesh::new(1).unwrap();
        doc.apply(&Xmesh::seed(SVG).unwrap()).unwrap();
        doc
    }

    fn body(doc: &Xmesh) -> String {
        doc.canonical().replacen(NS, "", 1)
    }

    #[test]
    fn insert() {
        let d = doc();
        d.edit(&[Op::Insert {
            parent: vec![],
            index: 1,
            xml: "<ellipse/>t".into(),
        }])
        .unwrap();
        assert_eq!(
            body(&d),
            r#"<svg><g id="g"><rect id="r"/></g><ellipse/>t<text>é😀z</text><circle id="c"/></svg>"#
        );
    }

    #[test]
    fn remove() {
        let d = doc();
        d.edit(&[Op::Remove { path: vec![0, 0] }]).unwrap();
        assert_eq!(
            body(&d),
            r#"<svg><g id="g"/><text>é😀z</text><circle id="c"/></svg>"#
        );
    }

    #[test]
    fn set_and_remove_attr() {
        let d = doc();
        d.edit(&[
            Op::SetAttr {
                path: vec![2],
                name: "fill".into(),
                value: "red".into(),
            },
            Op::SetAttr {
                path: vec![2],
                name: "inkscape:label".into(),
                value: "dot".into(),
            },
            Op::RemoveAttr {
                path: vec![2],
                name: "id".into(),
            },
        ])
        .unwrap();
        assert!(
            body(&d).contains(r#"<circle fill="red" inkscape:label="dot"/>"#),
            "{}",
            body(&d)
        );
    }

    #[test]
    fn expanded_attr_name_accepted() {
        let d = doc();
        let name = "{http://www.inkscape.org/namespaces/inkscape}label".to_string();
        d.edit(&[Op::SetAttr {
            path: vec![2],
            name,
            value: "x".into(),
        }])
        .unwrap();
        assert!(body(&d).contains(r#"inkscape:label="x""#));
    }

    #[test]
    fn unknown_prefix_refused() {
        let d = doc();
        let r = d.edit(&[Op::SetAttr {
            path: vec![2],
            name: "zz:a".into(),
            value: "x".into(),
        }]);
        assert!(matches!(r, Err(Error::Xml(_))));
    }

    #[test]
    fn insert_and_delete_text() {
        let d = doc();
        d.edit(&[Op::DeleteText {
            path: vec![1, 0],
            offset: 0,
            len: 1,
        }])
        .unwrap();
        assert!(body(&d).contains("<text>😀z</text>"));
    }

    #[test]
    fn insert_text_counts_utf16() {
        let d = doc();
        d.edit(&[Op::InsertText {
            path: vec![1, 0],
            offset: 3,
            text: "X".into(),
        }])
        .unwrap();
        assert!(body(&d).contains("<text>é😀Xz</text>"), "{}", body(&d));
    }

    #[test]
    fn text_offset_out_of_range() {
        let d = doc();
        let r = d.edit(&[Op::InsertText {
            path: vec![1, 0],
            offset: 9,
            text: "X".into(),
        }]);
        assert!(matches!(r, Err(Error::IndexOutOfRange(_, 9))));
    }

    #[test]
    fn move_reparents() {
        let d = doc();
        d.edit(&[Op::Move {
            path: vec![2],
            new_parent: vec![0],
            index: 0,
        }])
        .unwrap();
        assert_eq!(
            body(&d),
            r#"<svg><g id="g"><circle id="c"/><rect id="r"/></g><text>é😀z</text></svg>"#
        );
    }

    #[test]
    fn move_within_parent_counts_before_removal() {
        let d = doc();
        d.edit(&[Op::Move {
            path: vec![2],
            new_parent: vec![],
            index: 0,
        }])
        .unwrap();
        assert_eq!(
            body(&d),
            r#"<svg><circle id="c"/><g id="g"><rect id="r"/></g><text>é😀z</text></svg>"#
        );
        let d = doc();
        d.edit(&[Op::Move {
            path: vec![0],
            new_parent: vec![],
            index: 3,
        }])
        .unwrap();
        assert_eq!(
            body(&d),
            r#"<svg><text>é😀z</text><circle id="c"/><g id="g"><rect id="r"/></g></svg>"#
        );
    }

    #[test]
    fn move_into_own_subtree_refused() {
        let d = doc();
        let r = d.edit(&[Op::Move {
            path: vec![0],
            new_parent: vec![0, 0],
            index: 0,
        }]);
        assert!(matches!(r, Err(Error::MoveIntoSelf)));
        let r = d.edit(&[Op::Move {
            path: vec![0],
            new_parent: vec![0],
            index: 0,
        }]);
        assert!(matches!(r, Err(Error::MoveIntoSelf)));
    }

    #[test]
    fn root_cannot_be_removed_or_moved() {
        let d = doc();
        assert!(matches!(
            d.edit(&[Op::Remove { path: vec![] }]),
            Err(Error::BadPath(_))
        ));
    }

    #[test]
    fn bad_path() {
        let d = doc();
        assert!(matches!(
            d.edit(&[Op::Remove { path: vec![7] }]),
            Err(Error::BadPath(_))
        ));
        assert!(matches!(
            d.edit(&[Op::Remove {
                path: vec![0, 0, 0]
            }]),
            Err(Error::BadPath(_))
        ));
    }

    #[test]
    fn insert_index_out_of_range() {
        let d = doc();
        let r = d.edit(&[Op::Insert {
            parent: vec![0],
            index: 5,
            xml: "<a/>".into(),
        }]);
        assert!(matches!(r, Err(Error::IndexOutOfRange(_, 5))));
    }

    #[test]
    fn text_op_on_element() {
        let d = doc();
        let r = d.edit(&[Op::InsertText {
            path: vec![2],
            offset: 0,
            text: "x".into(),
        }]);
        assert!(matches!(r, Err(Error::NotText(_))));
    }

    #[test]
    fn element_op_on_text() {
        let d = doc();
        let r = d.edit(&[Op::SetAttr {
            path: vec![1, 0],
            name: "a".into(),
            value: "b".into(),
        }]);
        assert!(matches!(r, Err(Error::NotAnElement(_))));
    }

    #[test]
    fn failed_batch_changes_nothing() {
        let d = doc();
        let (sv, canon) = (d.state_vector(), d.canonical());
        let r = d.edit(&[
            Op::SetAttr {
                path: vec![2],
                name: "fill".into(),
                value: "red".into(),
            },
            Op::SetAttr {
                path: vec![0],
                name: "fill".into(),
                value: "blue".into(),
            },
            Op::Remove { path: vec![9] },
        ]);
        assert!(matches!(r, Err(Error::BadPath(_))));
        assert_eq!(d.state_vector(), sv);
        assert_eq!(d.canonical(), canon);
    }

    #[test]
    fn edit_returns_update() {
        let a = doc();
        let b = Xmesh::new(2).unwrap();
        b.apply(&a.diff(&b.state_vector()).unwrap()).unwrap();
        let update = a
            .edit(&[Op::Move {
                path: vec![2],
                new_parent: vec![0],
                index: 1,
            }])
            .unwrap();
        b.apply(&update).unwrap();
        assert_eq!(b.canonical(), a.canonical());
    }

    #[test]
    fn op_json_shape() {
        let op: Op =
            serde_json::from_str(r#"{"op":"set_attr","path":[0],"name":"fill","value":"red"}"#)
                .unwrap();
        assert_eq!(
            op,
            Op::SetAttr {
                path: vec![0],
                name: "fill".into(),
                value: "red".into()
            }
        );
    }
}
