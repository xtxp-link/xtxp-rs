//! The TUI's state: what's selected, folded and being typed. Pure: no terminal.

use std::collections::{BTreeMap, BTreeSet};
use xmesh::{IdNode, IdNodeKind, NodeId};

/// What the person is in the middle of.
#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    Browse,
    /// The selected element's attribute list, with the highlighted row.
    AttrList {
        index: usize,
    },
    EditValue {
        target: EditTarget,
        buffer: String,
    },
    AddAttrName {
        buffer: String,
    },
    AddAttrValue {
        name: String,
        buffer: String,
    },
    InsertXml {
        buffer: String,
    },
    /// A node marked to move; the next `m` on an element moves it there.
    MovePending(NodeId),
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditTarget {
    Attr(NodeId, String),
    Text(NodeId),
}

/// One visible row of the tree pane.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: NodeId,
    pub depth: usize,
    pub label: String,
    pub is_element: bool,
    pub has_children: bool,
}

#[derive(Debug, Clone)]
pub struct Model {
    pub doc: String,
    pub selected: Option<NodeId>,
    pub folded: BTreeSet<NodeId>,
    pub mode: InputMode,
    pub status: String,
    pub read_only: bool,
    pub quit: bool,
    /// Parent links from the last screen, to find a live ancestor.
    pub parents: BTreeMap<NodeId, NodeId>,
    /// Nodes to highlight, until the given time (ms).
    pub highlight_until: BTreeMap<NodeId, u64>,
}

impl Model {
    pub fn new(doc: &str, read_only: bool) -> Model {
        Model {
            doc: doc.to_string(),
            selected: None,
            folded: BTreeSet::new(),
            mode: InputMode::Browse,
            status: if read_only {
                "read-only: watching".into()
            } else {
                String::new()
            },
            read_only,
            quit: false,
            parents: BTreeMap::new(),
            highlight_until: BTreeMap::new(),
        }
    }
}

/// The root element: the first element that isn't a comment, PI, CDATA,
/// declaration or DOCTYPE.
pub fn root(tree: &[IdNode]) -> Option<&IdNode> {
    tree.iter().find(
        |n| matches!(&n.node, IdNodeKind::Element { name, .. } if !xmesh::names::is_reserved(name)),
    )
}

pub fn find(tree: &[IdNode], id: NodeId) -> Option<&IdNode> {
    for n in tree {
        if n.id == id {
            return Some(n);
        }
        if let IdNodeKind::Element { children, .. } = &n.node {
            if let Some(found) = find(children, id) {
                return Some(found);
            }
        }
    }
    None
}

/// An expanded name as a person reads it: `inkscape:label`, `fill`, or the local name.
pub fn display_name(name: &str) -> String {
    match xmesh::names::split(name) {
        (Some(uri), local) => match xmesh::names::FIXED_PREFIXES.iter().find(|(_, u)| *u == uri) {
            Some((prefix, _)) => format!("{prefix}:{local}"),
            None => local.to_string(),
        },
        (None, local) => local.to_string(),
    }
}

/// The visible rows: the root element's subtree, skipping folded children.
pub fn rows(model: &Model, tree: &[IdNode]) -> Vec<Row> {
    fn walk(model: &Model, n: &IdNode, depth: usize, out: &mut Vec<Row>) {
        let children = match &n.node {
            IdNodeKind::Element { children, .. } => children.as_slice(),
            IdNodeKind::Text(_) => &[],
        };
        out.push(Row {
            id: n.id,
            depth,
            label: label(n),
            is_element: matches!(n.node, IdNodeKind::Element { .. }),
            has_children: !children.is_empty(),
        });
        if !model.folded.contains(&n.id) {
            for c in children {
                walk(model, c, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(r) = root(tree) {
        walk(model, r, 0, &mut out);
    }
    out
}

/// A node's short label: `<rect id="r1">`, or text shortened to 40 characters.
pub fn label(n: &IdNode) -> String {
    match &n.node {
        IdNodeKind::Element { name, attrs, .. } => {
            let shown = display_name(name);
            match attrs.iter().find(|(k, _)| k == "id") {
                Some((_, id)) => format!("<{shown} id=\"{id}\">"),
                None => format!("<{shown}>"),
            }
        }
        IdNodeKind::Text(t) => {
            let one_line = t.replace('\n', "⏎");
            let short: String = one_line.chars().take(40).collect();
            if one_line.chars().count() > 40 {
                format!("\"{short}…\"")
            } else {
                format!("\"{short}\"")
            }
        }
    }
}
