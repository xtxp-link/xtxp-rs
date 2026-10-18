//! Reads a Yrs 0.19.2 document the way Xmesh reads a 0.28 one, so the two can
//! be compared through canonical XML.

use xmesh::{Element, Node};
use yrs019::types::xml::XmlOut;
use yrs019::{Doc, GetString, ReadTxn, Transact, Xml, XmlFragment};

/// The `xmesh` fragment of a Yrs 0.19.2 document, read as Xmesh nodes.
pub fn tree_019(doc: &Doc) -> Vec<Node> {
    let fragment = doc.get_or_insert_xml_fragment("xmesh");
    let txn = doc.transact();
    read(&txn, &fragment)
}

fn read<F: XmlFragment, T: ReadTxn>(txn: &T, parent: &F) -> Vec<Node> {
    parent
        .children(txn)
        .filter_map(|child| match child {
            XmlOut::Text(t) => Some(Node::Text(t.get_string(txn))),
            XmlOut::Element(e) => Some(Node::Element(Element {
                name: e.tag().to_string(),
                attrs: e.attributes(txn).map(|(k, v)| (k.to_string(), v)).collect(),
                children: read(txn, &e),
            })),
            XmlOut::Fragment(_) => None,
        })
        .collect()
}
