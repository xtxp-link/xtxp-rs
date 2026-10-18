//! The plain tree that sits between XML text, Yrs and canonical XML.

use crate::names::{expand, CDATA, COMMENT, PI};
use crate::Error;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

/// One XML node. Comments, processing instructions and CDATA are elements with
/// reserved names (see [`crate::names`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    Text(String),
}

/// An element: expanded name, attributes in source order, children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

/// Parses a whole document: exactly one root element, plus any comments and
/// processing instructions around it. The XML declaration and DOCTYPE are
/// dropped, as is whitespace outside the root element.
pub fn parse_document(xml: &str) -> Result<Vec<Node>, Error> {
    let nodes = parse_nodes(xml)?;
    let mut out = Vec::new();
    let mut roots = 0;
    for node in nodes {
        match node {
            Node::Text(t) if t.trim().is_empty() => {}
            Node::Text(t) => {
                return Err(Error::Xml(format!("text outside the root element: {t:?}")))
            }
            Node::Element(e) => {
                if e.name != COMMENT && e.name != PI {
                    roots += 1;
                }
                out.push(Node::Element(e));
            }
        }
    }
    if roots != 1 {
        return Err(Error::Xml(format!(
            "expected one root element, found {roots}"
        )));
    }
    Ok(out)
}

/// Parses one or more nodes to insert into a document. SVG is the default
/// namespace and the fixed prefixes (`inkscape`, `sodipodi`, `xlink`, `xml`)
/// are bound, so fragments needn't declare them.
pub fn parse_fragment(xml: &str) -> Result<Vec<Node>, Error> {
    let mut wrapped = format!(r#"<{WRAP} xmlns="{}""#, crate::names::SVG_NS);
    for (prefix, uri) in crate::names::FIXED_PREFIXES {
        if prefix != "xml" {
            wrapped.push_str(&format!(r#" xmlns:{prefix}="{uri}""#));
        }
    }
    wrapped.push('>');
    wrapped.push_str(xml);
    wrapped.push_str(&format!("</{WRAP}>"));
    match parse_nodes(&wrapped)?.pop() {
        Some(Node::Element(wrapper))
            if wrapper.name == expand(Some(crate::names::SVG_NS), WRAP) =>
        {
            Ok(wrapper.children)
        }
        _ => Err(Error::Xml("fragment escaped its wrapper".into())),
    }
}

/// Wrapper element used to pre-bind namespaces for fragments. It sits in the
/// SVG namespace (its own default) under a name no SVG document uses.
const WRAP: &str = "xtxp-fragment-wrapper";

fn xml_err(e: impl std::fmt::Display) -> Error {
    Error::Xml(e.to_string())
}

fn utf8(bytes: &[u8]) -> Result<String, Error> {
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(xml_err)
}

fn expanded(result: ResolveResult, local: &[u8]) -> Result<String, Error> {
    let local = std::str::from_utf8(local).map_err(xml_err)?;
    match result {
        ResolveResult::Unbound => Ok(local.to_string()),
        ResolveResult::Bound(ns) => Ok(expand(
            Some(std::str::from_utf8(ns.0).map_err(xml_err)?),
            local,
        )),
        ResolveResult::Unknown(prefix) => Err(Error::Xml(format!(
            "unbound prefix {:?}",
            String::from_utf8_lossy(&prefix)
        ))),
    }
}

fn reserved(name: &str, attrs: Vec<(String, String)>, text: String) -> Node {
    Node::Element(Element {
        name: name.to_string(),
        attrs,
        children: vec![Node::Text(text)],
    })
}

/// Reads every top-level node, resolving namespaces.
fn parse_nodes(xml: &str) -> Result<Vec<Node>, Error> {
    let mut reader = NsReader::from_str(xml);
    // Stack of open elements; the bottom entry collects top-level nodes.
    let mut stack: Vec<Element> = vec![Element {
        name: String::new(),
        attrs: vec![],
        children: vec![],
    }];
    loop {
        let (resolved, event) = reader.read_resolved_event().map_err(xml_err)?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let name = expanded(resolved, e.local_name().as_ref())?;
                let mut attrs = Vec::new();
                for attr in e.attributes() {
                    let attr = attr.map_err(xml_err)?;
                    if attr.key.as_namespace_binding().is_some() {
                        continue;
                    }
                    let (res, local) = reader.resolve_attribute(attr.key);
                    let key = expanded(res, local.as_ref())?;
                    let value = attr.unescape_value().map_err(xml_err)?.into_owned();
                    attrs.push((key, value));
                }
                let element = Element {
                    name,
                    attrs,
                    children: vec![],
                };
                if matches!(event, Event::Start(_)) {
                    stack.push(element);
                } else {
                    stack
                        .last_mut()
                        .expect("stack bottom")
                        .children
                        .push(Node::Element(element));
                }
            }
            Event::End(_) => {
                let done = stack.pop().expect("reader checks end names");
                stack
                    .last_mut()
                    .expect("stack bottom")
                    .children
                    .push(Node::Element(done));
            }
            Event::Text(t) => {
                let text = t.unescape().map_err(xml_err)?.into_owned();
                stack
                    .last_mut()
                    .expect("stack bottom")
                    .children
                    .push(Node::Text(text));
            }
            Event::CData(c) => {
                let text = utf8(&c.into_inner())?;
                stack
                    .last_mut()
                    .expect("stack bottom")
                    .children
                    .push(reserved(CDATA, vec![], text));
            }
            Event::Comment(c) => {
                let text = utf8(&c.into_inner())?;
                stack
                    .last_mut()
                    .expect("stack bottom")
                    .children
                    .push(reserved(COMMENT, vec![], text));
            }
            Event::PI(pi) => {
                let target = utf8(pi.target())?;
                let data = utf8(pi.content())?;
                let data = data.trim_start().to_string();
                let node = reserved(PI, vec![("target".to_string(), target)], data);
                stack.last_mut().expect("stack bottom").children.push(node);
            }
            Event::Decl(_) | Event::DocType(_) => {}
            Event::Eof => break,
        }
    }
    if stack.len() != 1 {
        return Err(Error::Xml(format!(
            "unclosed element {:?}",
            stack.last().map(|e| &e.name)
        )));
    }
    Ok(stack.pop().expect("stack bottom").children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::{INKSCAPE_NS, SVG_NS, XTXP_NS};

    fn el(node: &Node) -> &Element {
        match node {
            Node::Element(e) => e,
            Node::Text(t) => panic!("expected element, got text {t:?}"),
        }
    }

    #[test]
    fn expanded_names() {
        let xml = format!(
            r#"<svg xmlns="{SVG_NS}" xmlns:inkscape="{INKSCAPE_NS}"><g inkscape:label="L"/></svg>"#
        );
        let nodes = parse_document(&xml).unwrap();
        assert_eq!(nodes.len(), 1);
        let root = el(&nodes[0]);
        assert_eq!(root.name, format!("{{{SVG_NS}}}svg"));
        assert!(root.attrs.is_empty(), "xmlns declarations are not stored");
        let g = el(&root.children[0]);
        assert_eq!(g.name, format!("{{{SVG_NS}}}g"));
        assert_eq!(
            g.attrs,
            vec![(format!("{{{INKSCAPE_NS}}}label"), "L".to_string())]
        );
    }

    #[test]
    fn reserved_nodes() {
        let nodes = parse_document("<a><!--c--><?t d?><![CDATA[x<y]]></a>").unwrap();
        let a = el(&nodes[0]);
        assert_eq!(a.name, "a");
        let comment = el(&a.children[0]);
        assert_eq!(comment.name, COMMENT);
        assert_eq!(comment.name, format!("{{{XTXP_NS}}}comment"));
        assert_eq!(comment.children, vec![Node::Text("c".into())]);
        let pi = el(&a.children[1]);
        assert_eq!(pi.name, PI);
        assert_eq!(pi.attrs, vec![("target".to_string(), "t".to_string())]);
        assert_eq!(pi.children, vec![Node::Text("d".into())]);
        let cdata = el(&a.children[2]);
        assert_eq!(cdata.name, CDATA);
        assert_eq!(cdata.children, vec![Node::Text("x<y".into())]);
    }

    #[test]
    fn whitespace_kept() {
        let nodes = parse_document("<a> <b/>\n</a>").unwrap();
        let a = el(&nodes[0]);
        assert_eq!(a.children.len(), 3);
        assert_eq!(a.children[0], Node::Text(" ".into()));
        assert_eq!(el(&a.children[1]).name, "b");
        assert_eq!(a.children[2], Node::Text("\n".into()));
    }

    #[test]
    fn text_is_unescaped() {
        let nodes = parse_document("<a t=\"&quot;&amp;\">x &lt; y</a>").unwrap();
        let a = el(&nodes[0]);
        assert_eq!(a.attrs, vec![("t".to_string(), "\"&".to_string())]);
        assert_eq!(a.children, vec![Node::Text("x < y".into())]);
    }

    #[test]
    fn prolog_comment() {
        let nodes = parse_document("<?xml version=\"1.0\"?><!-- Created --><svg/>").unwrap();
        assert_eq!(nodes.len(), 2);
        let comment = el(&nodes[0]);
        assert_eq!(comment.name, COMMENT);
        assert_eq!(comment.children, vec![Node::Text(" Created ".into())]);
        assert_eq!(el(&nodes[1]).name, "svg");
    }

    #[test]
    fn whitespace_between_prolog_and_root_is_dropped() {
        let nodes = parse_document("<?xml version=\"1.0\"?>\n<!-- c -->\n<svg/>\n").unwrap();
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn two_roots_rejected() {
        assert!(matches!(parse_document("<a/><b/>"), Err(Error::Xml(_))));
    }

    #[test]
    fn no_root_rejected() {
        assert!(matches!(
            parse_document("<!-- only a comment -->"),
            Err(Error::Xml(_))
        ));
    }

    #[test]
    fn malformed_rejected() {
        assert!(matches!(parse_document("<a><b></a>"), Err(Error::Xml(_))));
        assert!(matches!(parse_document("<a>"), Err(Error::Xml(_))));
    }

    #[test]
    fn unbound_prefix_rejected() {
        assert!(matches!(
            parse_document("<a q:x=\"1\"/>"),
            Err(Error::Xml(_))
        ));
    }

    #[test]
    fn fragment_has_default_svg() {
        let nodes = parse_fragment("<rect inkscape:label=\"x\"/>").unwrap();
        assert_eq!(nodes.len(), 1);
        let rect = el(&nodes[0]);
        assert_eq!(rect.name, format!("{{{SVG_NS}}}rect"));
        assert_eq!(
            rect.attrs,
            vec![(format!("{{{INKSCAPE_NS}}}label"), "x".to_string())]
        );
    }

    #[test]
    fn fragment_may_hold_several_nodes() {
        let nodes = parse_fragment("<a/>text<!--c-->").unwrap();
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[1], Node::Text("text".into()));
    }

    #[test]
    fn same_uri_two_prefixes() {
        let xml = format!(r#"<svg xmlns="{SVG_NS}" xmlns:svg="{SVG_NS}"><svg:rect/><rect/></svg>"#);
        let nodes = parse_document(&xml).unwrap();
        let root = el(&nodes[0]);
        assert_eq!(el(&root.children[0]).name, format!("{{{SVG_NS}}}rect"));
        assert_eq!(el(&root.children[1]).name, format!("{{{SVG_NS}}}rect"));
    }
}
