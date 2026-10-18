//! Xmesh canonical form v0: the exact bytes every implementation hashes.

use crate::names::{
    is_reserved, split, CDATA, COMMENT, DOCTYPE, FIXED_PREFIXES, PI, SVG_NS, XML_DECL, XML_NS,
};
use crate::node::{Element, Node};
use crate::Xmesh;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

impl Xmesh {
    /// This replica's document in canonical form v0.
    pub fn canonical(&self) -> String {
        canonical(&self.tree())
    }

    /// SHA-256 of [`Xmesh::canonical`], lowercase hex.
    pub fn hash(&self) -> String {
        hash_of(&self.canonical())
    }
}

/// SHA-256 of `canonical`, lowercase hex.
pub fn hash_of(canonical: &str) -> String {
    hex(&Sha256::digest(canonical.as_bytes()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Writes nodes (a whole document: prolog, root element, epilogue) in
/// canonical form v0. Every namespace prefix is declared on the root element;
/// a default namespace is declared wherever it changes.
pub fn canonical(nodes: &[Node]) -> String {
    let prefixes = Prefixes::assign(nodes);
    let mut out = String::new();
    let mut root_done = false;
    for node in nodes {
        let is_root = !root_done && matches!(node, Node::Element(e) if !is_reserved(&e.name));
        write_node(&mut out, node, &prefixes, "", is_root);
        root_done |= is_root;
    }
    out
}

/// Prefixes for every namespace that needs one, by URI.
struct Prefixes {
    by_uri: BTreeMap<String, String>,
}

impl Prefixes {
    fn assign(nodes: &[Node]) -> Prefixes {
        let mut prefixes = Prefixes {
            by_uri: BTreeMap::new(),
        };
        let mut next = 1;
        prefixes.walk(nodes, &mut next);
        prefixes
    }

    fn walk(&mut self, nodes: &[Node], next: &mut u32) {
        for node in nodes {
            let Node::Element(e) = node else { continue };
            if is_reserved(&e.name) {
                continue;
            }
            if let (Some(uri), _) = split(&e.name) {
                if uri != SVG_NS {
                    self.need(uri, next);
                }
            }
            for (key, _) in sorted(&e.attrs) {
                if let (Some(uri), _) = split(key) {
                    self.need(uri, next);
                }
            }
            self.walk(&e.children, next);
        }
    }

    fn need(&mut self, uri: &str, next: &mut u32) {
        if self.by_uri.contains_key(uri) {
            return;
        }
        let prefix = if uri == SVG_NS {
            "svg".to_string()
        } else if let Some((p, _)) = FIXED_PREFIXES.iter().find(|(_, u)| *u == uri) {
            p.to_string()
        } else {
            let p = format!("ns{next}");
            *next += 1;
            p
        };
        self.by_uri.insert(uri.to_string(), prefix);
    }

    fn get(&self, uri: &str) -> &str {
        &self.by_uri[uri]
    }

    /// Declarations written on the root element, sorted by prefix; `xml` never.
    fn declarations(&self) -> Vec<(&str, &str)> {
        let mut decls: Vec<(&str, &str)> = self
            .by_uri
            .iter()
            .filter(|(uri, _)| uri.as_str() != XML_NS)
            .map(|(uri, p)| (p.as_str(), uri.as_str()))
            .collect();
        decls.sort();
        decls
    }
}

fn sorted(attrs: &[(String, String)]) -> Vec<&(String, String)> {
    let mut sorted: Vec<&(String, String)> = attrs.iter().collect();
    sorted.sort_by(|(a, _), (b, _)| {
        let (an, al) = split(a);
        let (bn, bl) = split(b);
        (an.unwrap_or(""), al).cmp(&(bn.unwrap_or(""), bl))
    });
    sorted
}

fn text_of(e: &Element) -> String {
    e.children
        .iter()
        .filter_map(|c| match c {
            Node::Text(t) => Some(t.as_str()),
            Node::Element(_) => None,
        })
        .collect()
}

fn write_node(out: &mut String, node: &Node, prefixes: &Prefixes, default_ns: &str, is_root: bool) {
    let e = match node {
        Node::Text(t) => return escape_text(out, t),
        Node::Element(e) => e,
    };
    match e.name.as_str() {
        COMMENT => {
            out.push_str("<!--");
            out.push_str(&text_of(e));
            out.push_str("-->");
            return;
        }
        PI => {
            let target = e
                .attrs
                .iter()
                .find(|(k, _)| k == "target")
                .map(|(_, v)| v.as_str());
            let data = text_of(e);
            out.push_str("<?");
            out.push_str(target.unwrap_or(""));
            if !data.is_empty() {
                out.push(' ');
                out.push_str(&data);
            }
            out.push_str("?>");
            return;
        }
        // The declaration isn't part of canonical form v0: the bytes are always UTF-8.
        XML_DECL => return,
        DOCTYPE => {
            out.push_str("<!DOCTYPE ");
            out.push_str(&text_of(e));
            out.push('>');
            return;
        }
        CDATA => {
            out.push_str("<![CDATA[");
            out.push_str(&text_of(e).replace("]]>", "]]]]><![CDATA[>"));
            out.push_str("]]>");
            return;
        }
        _ => {}
    }

    let (uri, local) = split(&e.name);
    let (qname, element_default) = match uri {
        Some(u) if u != SVG_NS => (format!("{}:{local}", prefixes.get(u)), None),
        other => (local.to_string(), Some(other.unwrap_or(""))),
    };
    out.push('<');
    out.push_str(&qname);
    let mut in_scope = default_ns;
    if let Some(ns) = element_default {
        if ns != default_ns {
            out.push_str(" xmlns=\"");
            escape_attr(out, ns);
            out.push('"');
            in_scope = ns;
        }
    }
    if is_root {
        for (prefix, uri) in prefixes.declarations() {
            out.push_str(&format!(" xmlns:{prefix}=\""));
            escape_attr(out, uri);
            out.push('"');
        }
    }
    for (key, value) in sorted(&e.attrs) {
        out.push(' ');
        match split(key) {
            (Some(u), l) => {
                out.push_str(prefixes.get(u));
                out.push(':');
                out.push_str(l);
            }
            (None, l) => out.push_str(l),
        }
        out.push_str("=\"");
        escape_attr(out, value);
        out.push('"');
    }
    if e.children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for child in &e.children {
        write_node(out, child, prefixes, in_scope, false);
    }
    out.push_str("</");
    out.push_str(&qname);
    out.push('>');
}

fn escape_text(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            c => out.push(c),
        }
    }
}

fn escape_attr(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::parse_document;

    const SVG: &str = "http://www.w3.org/2000/svg";
    const INK: &str = "http://www.inkscape.org/namespaces/inkscape";
    const XLINK: &str = "http://www.w3.org/1999/xlink";

    fn canon(xml: &str) -> String {
        canonical(&parse_document(xml).unwrap())
    }

    #[test]
    fn sorted_attrs() {
        let xml = format!(
            r##"<svg xmlns="{SVG}" xmlns:inkscape="{INK}" xmlns:xlink="{XLINK}"><rect y="1" x="2" inkscape:label="a" xlink:href="#b"/></svg>"##
        );
        assert_eq!(
            canon(&xml),
            format!(
                r##"<svg xmlns="{SVG}" xmlns:inkscape="{INK}" xmlns:xlink="{XLINK}"><rect x="2" y="1" inkscape:label="a" xlink:href="#b"/></svg>"##
            )
        );
    }

    #[test]
    fn declarations_on_root() {
        // Declared on a child in the source, unused prefix dropped, xml: never declared.
        let xml = format!(
            r#"<svg xmlns="{SVG}" xmlns:unused="urn:u"><g xmlns:sodipodi="http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd" sodipodi:role="line" xml:space="preserve"/><inkscape:x xmlns:inkscape="{INK}"/></svg>"#
        );
        assert_eq!(
            canon(&xml),
            format!(
                r#"<svg xmlns="{SVG}" xmlns:inkscape="{INK}" xmlns:sodipodi="http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd"><g sodipodi:role="line" xml:space="preserve"/><inkscape:x/></svg>"#
            )
        );
    }

    #[test]
    fn unknown_namespaces_numbered() {
        let xml = format!(
            r#"<svg xmlns="{SVG}" xmlns:b="urn:b" xmlns:a="urn:a"><b:x a:y="1"/><a:z/></svg>"#
        );
        assert_eq!(
            canon(&xml),
            format!(
                r#"<svg xmlns="{SVG}" xmlns:ns1="urn:b" xmlns:ns2="urn:a"><ns1:x ns2:y="1"/><ns2:z/></svg>"#
            )
        );
    }

    #[test]
    fn empty_element_self_closes() {
        assert_eq!(canon("<a><g></g></a>"), "<a><g/></a>");
    }

    #[test]
    fn no_namespace_element() {
        let xml = format!(r#"<svg xmlns="{SVG}"><foo xmlns=""><bar/></foo><rect/></svg>"#);
        assert_eq!(
            canon(&xml),
            format!(r#"<svg xmlns="{SVG}"><foo xmlns=""><bar/></foo><rect/></svg>"#)
        );
    }

    #[test]
    fn svg_element_under_no_namespace_element() {
        let xml = format!(r#"<svg xmlns="{SVG}"><foo xmlns=""><rect xmlns="{SVG}"/></foo></svg>"#);
        assert_eq!(
            canon(&xml),
            format!(r#"<svg xmlns="{SVG}"><foo xmlns=""><rect xmlns="{SVG}"/></foo></svg>"#)
        );
    }

    #[test]
    fn svg_namespaced_attribute_gets_svg_prefix() {
        let xml = format!(r#"<svg xmlns="{SVG}" xmlns:s="{SVG}"><rect s:x="1"/></svg>"#);
        assert_eq!(
            canon(&xml),
            format!(r#"<svg xmlns="{SVG}" xmlns:svg="{SVG}"><rect svg:x="1"/></svg>"#)
        );
    }

    #[test]
    fn reserved_written_back() {
        let xml =
            "<?xml version=\"1.0\"?>\n<!-- made -->\n<a><!--c--><?t d?><?bare?><![CDATA[x<y]]></a>";
        assert_eq!(
            canon(xml),
            "<!-- made --><a><!--c--><?t d?><?bare?><![CDATA[x<y]]></a>"
        );
    }

    #[test]
    fn doctype_written_declaration_omitted() {
        assert_eq!(
            canon(
                r#"<?xml version="1.0" standalone="no"?><!DOCTYPE svg PUBLIC "a" "b"><!--c--><svg/>"#
            ),
            r#"<!DOCTYPE svg PUBLIC "a" "b"><!--c--><svg/>"#
        );
    }

    #[test]
    fn whitespace_inside_kept() {
        assert_eq!(canon("<a>\n  <b> x </b>\n</a>"), "<a>\n  <b> x </b>\n</a>");
    }

    #[test]
    fn escaping() {
        assert_eq!(
            canon(r#"<a t="&quot;&amp;&lt;&gt;">a&amp;b&lt;c&gt;d</a>"#),
            r#"<a t="&quot;&amp;&lt;>">a&amp;b&lt;c>d</a>"#
        );
        assert_eq!(
            canon("<a><![CDATA[a]]]]><![CDATA[>b]]></a>"),
            "<a><![CDATA[a]]]]><![CDATA[>b]]></a>",
            "two adjacent CDATA sections stay two"
        );
    }

    #[test]
    fn cdata_containing_terminator_is_split() {
        let nodes = vec![Node::Element(crate::Element {
            name: "a".into(),
            attrs: vec![],
            children: vec![Node::Element(crate::Element {
                name: crate::names::CDATA.into(),
                attrs: vec![],
                children: vec![Node::Text("a]]>b".into())],
            })],
        })];
        assert_eq!(canonical(&nodes), "<a><![CDATA[a]]]]><![CDATA[>b]]></a>");
    }

    #[test]
    fn hash_is_sha256_hex() {
        // FIPS 180-2 test vector for "abc".
        assert_eq!(
            hash_of("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hash_of(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn xmesh_canonical_matches_tree() {
        let xml = r#"<svg xmlns="http://www.w3.org/2000/svg"><rect y="1" x="2"/></svg>"#;
        let doc = Xmesh::new(1).unwrap();
        doc.apply(&Xmesh::seed(xml).unwrap()).unwrap();
        assert_eq!(doc.canonical(), canon(xml));
        assert_eq!(doc.hash(), hash_of(&canon(xml)));
    }
}
