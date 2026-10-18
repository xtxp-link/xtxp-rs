//! The plain tree that sits between XML text, Yrs and canonical XML.

use crate::names::{expand, is_reserved, CDATA, COMMENT, DOCTYPE, PI, XML_DECL};
use crate::Error;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use std::collections::BTreeMap;

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
    let expanded = expand_doctype_entities(xml)?;
    let nodes = parse_nodes(&expanded, true)?;
    let mut out = Vec::new();
    let mut roots = 0;
    for node in nodes {
        match node {
            Node::Text(t) if t.trim().is_empty() => {}
            Node::Text(t) => {
                return Err(Error::Xml(format!("text outside the root element: {t:?}")))
            }
            Node::Element(e) => {
                if !is_reserved(&e.name) {
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
    match parse_nodes(&wrapped, false)?.pop() {
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

/// Entities declared in a DOCTYPE's internal subset (`<!ENTITY name "value">`),
/// substituted into the rest of the document before parsing, so they also work
/// in namespace declarations. References inside comments, CDATA and processing
/// instructions are left alone, as XML requires. Parameter and external
/// entities aren't supported: a reference to one stays unresolved and fails.
fn expand_doctype_entities(xml: &str) -> Result<std::borrow::Cow<'_, str>, Error> {
    let Some(start) = xml.find("<!DOCTYPE") else {
        return Ok(xml.into());
    };
    let end = doctype_end(xml, start)?;
    let entities = declared_entities(&xml[start..end]);
    if entities.is_empty() {
        return Ok(xml.into());
    }
    let mut out = String::with_capacity(xml.len());
    out.push_str(&xml[..end]);
    let rest = &xml[end..];
    let mut i = 0;
    while i < rest.len() {
        let tail = &rest[i..];
        let skip = [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")]
            .iter()
            .find(|(open, _)| tail.starts_with(open))
            .map(|(_, close)| tail.find(close).map_or(tail.len(), |j| j + close.len()));
        if let Some(len) = skip {
            out.push_str(&tail[..len]);
            i += len;
            continue;
        }
        if tail.starts_with('&') {
            if let Some(semi) = tail.find(';') {
                if let Some(value) = entities.get(&tail[1..semi]) {
                    out.push_str(value);
                    i += semi + 1;
                    continue;
                }
            }
        }
        let c = tail.chars().next().expect("non-empty");
        out.push(c);
        i += c.len_utf8();
    }
    Ok(out.into())
}

/// Byte offset just past the DOCTYPE that starts at `start`, allowing for an
/// internal subset in brackets and quoted strings.
fn doctype_end(xml: &str, start: usize) -> Result<usize, Error> {
    let mut depth = 0;
    let mut quote = None;
    for (i, c) in xml[start..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '[') => depth += 1,
            (None, ']') => depth -= 1,
            (None, '>') if depth == 0 => return Ok(start + i + 1),
            _ => {}
        }
    }
    Err(Error::Xml("unterminated DOCTYPE".into()))
}

fn declared_entities(doctype: &str) -> BTreeMap<String, String> {
    let mut entities = BTreeMap::new();
    for decl in doctype.split("<!ENTITY").skip(1) {
        let decl = decl.trim_start();
        if decl.starts_with('%') {
            continue;
        }
        let Some((name, rest)) = decl.split_once(char::is_whitespace) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            continue;
        };
        if let Some(value) = rest[1..].split(q).next() {
            entities.insert(name.to_string(), value.to_string());
        }
    }
    entities
}

/// Reads every top-level node, resolving namespaces. The declaration and
/// DOCTYPE are allowed only in a whole document.
fn parse_nodes(xml: &str, document: bool) -> Result<Vec<Node>, Error> {
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
            Event::Decl(_) | Event::DocType(_) if !document => {
                return Err(Error::Xml(
                    "a fragment can't hold a declaration or DOCTYPE".into(),
                ));
            }
            Event::Decl(d) => {
                let mut attrs =
                    vec![("version".to_string(), utf8(&d.version().map_err(xml_err)?)?)];
                if let Some(standalone) = d.standalone() {
                    attrs.push((
                        "standalone".to_string(),
                        utf8(&standalone.map_err(xml_err)?)?,
                    ));
                }
                let node = Node::Element(Element {
                    name: XML_DECL.to_string(),
                    attrs,
                    children: vec![],
                });
                stack.last_mut().expect("stack bottom").children.push(node);
            }
            Event::DocType(t) => {
                let text = utf8(&t.into_inner())?.trim().to_string();
                stack
                    .last_mut()
                    .expect("stack bottom")
                    .children
                    .push(reserved(DOCTYPE, vec![], text));
            }
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
    use crate::names::{DOCTYPE, INKSCAPE_NS, SVG_NS, XML_DECL, XTXP_NS};

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
        assert_eq!(nodes.len(), 3);
        assert_eq!(el(&nodes[0]).name, XML_DECL);
        let comment = el(&nodes[1]);
        assert_eq!(comment.name, COMMENT);
        assert_eq!(comment.children, vec![Node::Text(" Created ".into())]);
        assert_eq!(el(&nodes[2]).name, "svg");
    }

    #[test]
    fn whitespace_between_prolog_and_root_is_dropped() {
        let nodes = parse_document("<?xml version=\"1.0\"?>\n<!-- c -->\n<svg/>\n").unwrap();
        assert_eq!(nodes.len(), 3);
    }

    #[test]
    fn declaration_kept_without_encoding() {
        let nodes =
            parse_document("<?xml version=\"1.0\" encoding=\"ISO-8859-1\" standalone=\"no\"?><a/>")
                .unwrap();
        let decl = el(&nodes[0]);
        assert_eq!(decl.name, XML_DECL);
        assert_eq!(decl.name, format!("{{{XTXP_NS}}}xml-decl"));
        assert_eq!(
            decl.attrs,
            vec![
                ("version".to_string(), "1.0".to_string()),
                ("standalone".to_string(), "no".to_string())
            ]
        );
        assert!(decl.children.is_empty());
    }

    #[test]
    fn doctype_kept_verbatim() {
        let xml = r#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd"><svg/>"#;
        let nodes = parse_document(xml).unwrap();
        let doctype = el(&nodes[0]);
        assert_eq!(doctype.name, DOCTYPE);
        assert_eq!(doctype.name, format!("{{{XTXP_NS}}}doctype"));
        assert_eq!(
            doctype.children,
            vec![Node::Text(
                r#"svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd""#.into()
            )]
        );
    }

    const ILLUSTRATOR: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n",
        "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\" [\n",
        "\t<!ENTITY ns_extend \"http://ns.adobe.com/Extensibility/1.0/\">\n",
        "\t<!ENTITY ns_svg \"http://www.w3.org/2000/svg\">\n",
        "]>\n",
        "<svg xmlns:x=\"&ns_extend;\" xmlns=\"&ns_svg;\"><x:pgf t=\"&ns_svg;\">a &ns_svg; b</x:pgf>",
        "<!-- &ns_svg; --><![CDATA[&ns_svg;]]></svg>"
    );

    #[test]
    fn doctype_entities_expanded() {
        let nodes = parse_document(ILLUSTRATOR).unwrap();
        assert_eq!(el(&nodes[0]).name, XML_DECL);
        let doctype = el(&nodes[1]);
        assert_eq!(doctype.name, DOCTYPE);
        match &doctype.children[0] {
            Node::Text(t) => assert!(
                t.contains("<!ENTITY ns_svg \"http://www.w3.org/2000/svg\">"),
                "{t}"
            ),
            other => panic!("{other:?}"),
        }
        let svg = el(&nodes[2]);
        assert_eq!(svg.name, format!("{{{SVG_NS}}}svg"));
        let pgf = el(&svg.children[0]);
        assert_eq!(pgf.name, "{http://ns.adobe.com/Extensibility/1.0/}pgf");
        assert_eq!(pgf.attrs, vec![("t".to_string(), SVG_NS.to_string())]);
        assert_eq!(pgf.children, vec![Node::Text(format!("a {SVG_NS} b"))]);
        // Entity references aren't recognised inside comments or CDATA.
        assert_eq!(
            el(&svg.children[1]).children,
            vec![Node::Text(" &ns_svg; ".into())]
        );
        assert_eq!(
            el(&svg.children[2]).children,
            vec![Node::Text("&ns_svg;".into())]
        );
    }

    #[test]
    fn unknown_entity_rejected() {
        assert!(matches!(
            parse_document("<a>&nope;</a>"),
            Err(Error::Xml(_))
        ));
    }

    #[test]
    fn fragment_rejects_declaration_and_doctype() {
        assert!(matches!(
            parse_fragment("<!DOCTYPE x><a/>"),
            Err(Error::Xml(_))
        ));
        assert!(matches!(
            parse_fragment("<?xml version=\"1.0\"?><a/>"),
            Err(Error::Xml(_))
        ));
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
