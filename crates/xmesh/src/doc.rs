//! An Xmesh document: a Yrs document holding XML in Yrs's XML types.

use crate::node::{parse_document, Element, Node};
use crate::Error;
use yrs::types::xml::XmlOut;
use yrs::updates::decoder::Decode;
use yrs::{
    Any, Doc, GetString, OffsetKind, Options, Out, ReadTxn, StateVector, Transact, TransactionMut,
    Update, Xml, XmlElementPrelim, XmlFragment, XmlFragmentRef, XmlTextPrelim,
};

/// Name of the Yrs root XML fragment that holds the document.
pub(crate) const FRAGMENT: &str = "xmesh";

/// One replica of an XML document, held in Yrs's XML types.
pub struct Xmesh {
    pub(crate) doc: Doc,
}

impl Xmesh {
    /// Client ID used only to make seed updates.
    pub const SEED_CLIENT: u64 = 0;

    /// An empty replica editing as `client_id`. Client ID 0 is reserved.
    pub fn new(client_id: u64) -> Result<Xmesh, Error> {
        if client_id == Self::SEED_CLIENT {
            return Err(Error::ReservedClientId);
        }
        if client_id >= 1 << 53 {
            return Err(Error::ClientIdTooLarge(client_id));
        }
        Ok(Xmesh {
            doc: new_doc(client_id),
        })
    }

    /// The seed update for a document: the whole of `xml`, made by client 0,
    /// byte-identical wherever it's made.
    pub fn seed(xml: &str) -> Result<Vec<u8>, Error> {
        let nodes = parse_document(xml)?;
        let doc = new_doc(Self::SEED_CLIENT);
        let fragment = doc.get_or_insert_xml_fragment(FRAGMENT);
        let mut txn = doc.transact_mut();
        write_nodes(&mut txn, &fragment, 0, &nodes);
        Ok(txn.encode_state_as_update_v1(&StateVector::default()))
    }

    pub fn client_id(&self) -> u64 {
        self.doc.client_id().get()
    }

    /// Applies a Yrs v1 update. Applying the same update again changes nothing.
    pub fn apply(&self, update: &[u8]) -> Result<(), Error> {
        let update = Update::decode_v1(update).map_err(|e| Error::Update(e.to_string()))?;
        self.doc
            .transact_mut()
            .apply_update(update)
            .map_err(|e| Error::Update(e.to_string()))
    }

    /// This replica's state vector, v1-encoded.
    pub fn state_vector(&self) -> Vec<u8> {
        use yrs::updates::encoder::Encode;
        self.doc.transact().state_vector().encode_v1()
    }

    /// Everything this replica has that a peer with `state_vector` lacks.
    pub fn diff(&self, state_vector: &[u8]) -> Result<Vec<u8>, Error> {
        let sv = StateVector::decode_v1(state_vector).map_err(|e| Error::Update(e.to_string()))?;
        Ok(self.doc.transact().encode_diff_v1(&sv))
    }

    /// The live document as a tree: the root element plus the comments and
    /// processing instructions around it. Attributes come back in canonical
    /// order (namespace URI, then local name), because Yrs keeps them unordered.
    pub fn tree(&self) -> Vec<Node> {
        let fragment = self.fragment();
        let txn = self.doc.transact();
        read_nodes(&txn, &fragment)
    }

    pub(crate) fn fragment(&self) -> XmlFragmentRef {
        self.doc.get_or_insert_xml_fragment(FRAGMENT)
    }
}

pub(crate) fn new_doc(client_id: u64) -> Doc {
    Doc::with_options(Options {
        skip_gc: true,
        offset_kind: OffsetKind::Utf16,
        ..Options::with_client_id(yrs::block::ClientID::new(client_id))
    })
}

/// Inserts `nodes` under `parent` starting at `index`, attributes in order.
pub(crate) fn write_nodes<F: XmlFragment>(
    txn: &mut TransactionMut,
    parent: &F,
    index: u32,
    nodes: &[Node],
) {
    for (i, node) in nodes.iter().enumerate() {
        let at = index + i as u32;
        match node {
            Node::Text(text) => {
                parent.insert(txn, at, XmlTextPrelim::new(text.as_str()));
            }
            Node::Element(e) => {
                let element = parent.insert(txn, at, XmlElementPrelim::empty(e.name.as_str()));
                for (key, value) in &e.attrs {
                    element.insert_attribute(txn, key.as_str(), value.clone());
                }
                write_nodes(txn, &element, 0, &e.children);
            }
        }
    }
}

/// Reads the live children of `parent` as nodes.
pub(crate) fn read_nodes<F: XmlFragment, T: ReadTxn>(txn: &T, parent: &F) -> Vec<Node> {
    parent
        .children(txn)
        .filter_map(|child| match child {
            XmlOut::Text(text) => Some(Node::Text(text.get_string(txn))),
            XmlOut::Element(e) => {
                let mut attrs: Vec<(String, String)> = e
                    .attributes(txn)
                    .map(|(k, v)| (k.to_string(), out_to_string(v)))
                    .collect();
                sort_attrs(&mut attrs);
                Some(Node::Element(Element {
                    name: e.tag().to_string(),
                    attrs,
                    children: read_nodes(txn, &e),
                }))
            }
            XmlOut::Fragment(_) => None,
        })
        .collect()
}

/// Sorts attributes in canonical order: namespace URI (none first), then local name.
pub(crate) fn sort_attrs(attrs: &mut [(String, String)]) {
    attrs.sort_by(|(a, _), (b, _)| {
        let (an, al) = crate::names::split(a);
        let (bn, bl) = crate::names::split(b);
        (an.unwrap_or(""), al).cmp(&(bn.unwrap_or(""), bl))
    });
}

fn out_to_string(value: Out) -> String {
    match value {
        Out::Any(Any::String(s)) => s.to_string(),
        Out::Any(other) => other.to_string(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::{COMMENT, SVG_NS};

    /// `parse_document` with attributes sorted the way `tree()` returns them.
    fn parsed(xml: &str) -> Vec<Node> {
        fn sort(nodes: &mut [Node]) {
            for node in nodes {
                if let Node::Element(e) = node {
                    sort_attrs(&mut e.attrs);
                    sort(&mut e.children);
                }
            }
        }
        let mut nodes = parse_document(xml).unwrap();
        sort(&mut nodes);
        nodes
    }

    const X: &str = r#"<svg xmlns="http://www.w3.org/2000/svg"><rect z="1" b="2" y="3" a="4" x="5"/>hi<!--c--></svg>"#;

    #[test]
    fn seed_is_deterministic() {
        assert_eq!(Xmesh::seed(X).unwrap(), Xmesh::seed(X).unwrap());
    }

    #[test]
    fn seed_round_trips() {
        let doc = Xmesh::new(7).unwrap();
        doc.apply(&Xmesh::seed(X).unwrap()).unwrap();
        assert_eq!(doc.tree(), parsed(X));
    }

    #[test]
    fn seed_twice_is_noop() {
        let seed = Xmesh::seed(X).unwrap();
        let once = Xmesh::new(7).unwrap();
        once.apply(&seed).unwrap();
        let twice = Xmesh::new(7).unwrap();
        twice.apply(&seed).unwrap();
        twice.apply(&seed).unwrap();
        assert_eq!(twice.tree(), once.tree());
        assert_eq!(twice.state_vector(), once.state_vector());
    }

    #[test]
    fn reserved_client_refused() {
        assert!(matches!(Xmesh::new(0), Err(Error::ReservedClientId)));
        assert!(matches!(
            Xmesh::new(1 << 53),
            Err(Error::ClientIdTooLarge(_))
        ));
        assert_eq!(Xmesh::new(9).unwrap().client_id(), 9);
    }

    #[test]
    fn diff_brings_peer_up_to_date() {
        let a = Xmesh::new(1).unwrap();
        a.apply(&Xmesh::seed(X).unwrap()).unwrap();
        let b = Xmesh::new(2).unwrap();
        b.apply(&a.diff(&b.state_vector()).unwrap()).unwrap();
        assert_eq!(b.tree(), a.tree());
    }

    #[test]
    fn bad_update_is_error() {
        let doc = Xmesh::new(1).unwrap();
        assert!(matches!(doc.apply(&[0xff, 0xff]), Err(Error::Update(_))));
        assert!(matches!(doc.diff(&[0xff, 0xff]), Err(Error::Update(_))));
    }

    #[test]
    fn bad_seed_is_error() {
        assert!(matches!(Xmesh::seed("<a><b></a>"), Err(Error::Xml(_))));
    }

    #[test]
    fn seed_real_inkscape_file() {
        let xml = include_str!("../tests/fixtures/inkscape-blank.svg");
        let doc = Xmesh::new(3).unwrap();
        doc.apply(&Xmesh::seed(xml).unwrap()).unwrap();
        let tree = doc.tree();
        assert_eq!(tree.len(), 2);
        match (&tree[0], &tree[1]) {
            (Node::Element(c), Node::Element(svg)) => {
                assert_eq!(c.name, COMMENT);
                assert_eq!(svg.name, format!("{{{SVG_NS}}}svg"));
                assert!(svg
                    .children
                    .iter()
                    .any(|n| matches!(n, Node::Text(t) if t == "\n  ")));
            }
            other => panic!("unexpected tree {other:?}"),
        }
        assert_eq!(tree, parsed(xml));
    }
}

#[cfg(test)]
mod golden {
    use super::*;

    /// Run twice in fresh processes, this pins the seed's bytes across runs,
    /// not only within one process.
    #[test]
    fn seed_bytes_are_stable_across_runs() {
        let seed = Xmesh::seed(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><rect z="1" b="2" y="3"/></svg>"#,
        )
        .unwrap();
        let hex: String = seed.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, GOLDEN);
    }

    const GOLDEN: &str = "01050000070105786d657368031f7b687474703a2f2f7777772e77332e6f72672f323030302f7376677d7376670700000003207b687474703a2f2f7777772e77332e6f72672f323030302f7376677d7265637428000001017a01770131280000010162017701322800000101790177013300";
}
