//! Xmesh: XTXP's pure core. XML documents held in Yrs's XML types.
//!
//! No networking, file access, clocks or threads: everything that touches the
//! world is an adapter outside this crate.

mod canonical;
mod doc;
mod edit;
mod ids;
pub mod names;
mod node;
mod snapshot;
pub mod vector;

pub use canonical::{canonical, hash_of};
pub use doc::Xmesh;
pub use edit::Op;
pub use ids::{IdNode, IdNodeKind, NodeId};
pub use node::{parse_document, parse_fragment, Element, Node};
pub use snapshot::Snapshot;

use std::fmt;

/// Every fallible call in Xmesh returns this error. Paths are lists of child
/// indexes starting at the root element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// XML that isn't well-formed, or a document that isn't exactly one root element.
    Xml(String),
    /// Client ID 0 is reserved for seed updates.
    ReservedClientId,
    /// Client IDs are at most 32 bits: Yjs, and Yrs 0.19 (y-sweet's), read them as 32 bits.
    ClientIdTooLarge(u64),
    /// A path that doesn't resolve to a node.
    BadPath(Vec<u32>),
    /// An element operation on a node that isn't an element.
    NotAnElement(Vec<u32>),
    /// A text operation on a node that isn't text.
    NotText(Vec<u32>),
    /// An index or offset past the end of the node at the path.
    IndexOutOfRange(Vec<u32>, u32),
    /// A move into the moved node's own subtree.
    MoveIntoSelf,
    /// An update that Yrs couldn't decode or apply.
    Update(String),
    /// A vector that is malformed or whose expectations failed.
    Vector(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Xml(msg) => write!(f, "XML error: {msg}"),
            Error::ReservedClientId => write!(f, "client ID 0 is reserved for seeds"),
            Error::ClientIdTooLarge(id) => write!(f, "client ID {id} is more than 32 bits"),
            Error::BadPath(p) => write!(f, "no node at path {p:?}"),
            Error::NotAnElement(p) => write!(f, "node at path {p:?} is not an element"),
            Error::NotText(p) => write!(f, "node at path {p:?} is not text"),
            Error::IndexOutOfRange(p, i) => write!(f, "index {i} out of range at path {p:?}"),
            Error::MoveIntoSelf => write!(f, "cannot move a node into its own subtree"),
            Error::Update(msg) => write!(f, "update error: {msg}"),
            Error::Vector(msg) => write!(f, "vector: {msg}"),
        }
    }
}

impl std::error::Error for Error {}
