//! Nodes, regions and pages as HTML.

use std::collections::{BTreeMap, BTreeSet};
use xmesh::{names, IdNode, IdNodeKind, NodeId};

/// The marker name of a node: `n{client}-{clock}`.
pub fn marker_name(id: NodeId) -> String {
    format!("n{}-{}", id.client, id.clock)
}

/// Naming state shared by a page and every later patch to it: page-unique,
/// CSS-safe `view-transition-name`s, kept for each node's life so a node's
/// patches use the name its first rendering used, and prefixes for unknown
/// namespaces.
#[derive(Debug, Default)]
pub struct TransitionNames {
    by_node: BTreeMap<NodeId, String>,
    taken: BTreeSet<String>,
    prefixes: BTreeMap<String, String>,
}

impl TransitionNames {
    #[cfg(test)]
    pub fn new() -> TransitionNames {
        TransitionNames::default()
    }

    fn name_for(&mut self, id: NodeId, attrs: &[(String, String)]) -> String {
        if let Some(name) = self.by_node.get(&id) {
            return name.clone();
        }
        let base = match attrs.iter().find(|(k, _)| k == "id") {
            Some((_, v)) if !v.is_empty() => css_ident(v),
            _ => marker_name(id),
        };
        let mut name = base.clone();
        let mut n = 2;
        while self.taken.contains(&name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        self.taken.insert(name.clone());
        self.by_node.insert(id, name.clone());
        name
    }

    fn prefix_for(&mut self, uri: &str) -> String {
        let next = self.prefixes.len() + 1;
        self.prefixes
            .entry(uri.to_string())
            .or_insert_with(|| format!("ns{next}"))
            .clone()
    }
}

/// `value` as a CSS identifier: other characters become `-`, and a leading
/// digit gets a `_`.
fn css_ident(value: &str) -> String {
    let mut out: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let starts_badly = out.starts_with(|c: char| c.is_ascii_digit())
        || out.starts_with("--")
        || (out.starts_with('-') && out[1..].starts_with(|c: char| c.is_ascii_digit()));
    if starts_badly {
        out.insert(0, '_');
    }
    out
}

fn escape_attr(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_text(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// How a qualified name is written in the page: SVG names bare, the fixed
/// prefixes as usual, unknown namespaces as `ns{n}`.
fn written_name(expanded: &str, names: &mut TransitionNames) -> String {
    match names::split(expanded) {
        (None, local) => local.to_string(),
        (Some(names::SVG_NS), local) => local.to_string(),
        (Some(uri), local) => match names::FIXED_PREFIXES.iter().find(|(_, u)| *u == uri) {
            Some((prefix, _)) => format!("{prefix}:{local}"),
            None => format!("{}:{local}", names.prefix_for(uri)),
        },
    }
}

fn text_of(children: &[IdNode]) -> String {
    children
        .iter()
        .filter_map(|c| match &c.node {
            IdNodeKind::Text(t) => Some(t.as_str()),
            IdNodeKind::Element { .. } => None,
        })
        .collect()
}

/// A node's subtree, every element and text node inside its markers.
pub fn render_node(n: &IdNode, names: &mut TransitionNames) -> String {
    render_in(n, names)
}

fn render_in(n: &IdNode, names: &mut TransitionNames) -> String {
    let mut out = format!("<?start name=\"{}\">", marker_name(n.id));
    out.push_str(&render_bare(n, names, ""));
    out.push_str("<?end>");
    out
}

/// A node without its own markers; `extra` adds attributes (the root's
/// namespace declarations).
fn render_bare(n: &IdNode, names: &mut TransitionNames, extra: &str) -> String {
    match &n.node {
        IdNodeKind::Text(t) => escape_text(t),
        IdNodeKind::Element { name, children, .. } if name == names::COMMENT => {
            format!("<!--{}-->", text_of(children).replace("--", "- -"))
        }
        IdNodeKind::Element { name, attrs, .. } if name == names::PI => {
            let target = attrs
                .iter()
                .find(|(k, _)| k == "target")
                .map(|(_, v)| v.as_str());
            format!("<!--?{} -->", target.unwrap_or("").replace("--", "- -"))
        }
        IdNodeKind::Element { name, children, .. } if names::is_reserved(name) => {
            escape_text(&text_of(children))
        }
        IdNodeKind::Element {
            name,
            attrs,
            children,
        } => {
            let tag = written_name(name, names);
            let transition = names.name_for(n.id, attrs);
            let mut out = format!("<{tag}{extra}");
            let mut styled = false;
            for (k, v) in attrs {
                let key = written_name(k, names);
                if key == "style" {
                    styled = true;
                    out.push_str(&format!(
                        " style=\"{}; view-transition-name: {transition}\"",
                        escape_attr(v)
                    ));
                } else {
                    out.push_str(&format!(" {key}=\"{}\"", escape_attr(v)));
                }
            }
            if !styled {
                out.push_str(&format!(" style=\"view-transition-name: {transition}\""));
            }
            if children.is_empty() {
                out.push_str("/>");
            } else {
                out.push('>');
                for c in children {
                    out.push_str(&render_in(c, names));
                }
                out.push_str(&format!("</{tag}>"));
            }
            out
        }
    }
}

/// The region marker `doc` around the root element as inline SVG, or empty.
pub fn render_region(tree: &[IdNode], names: &mut TransitionNames) -> String {
    let root = tree
        .iter()
        .find(|n| matches!(&n.node, IdNodeKind::Element { name, .. } if !names::is_reserved(name)));
    let Some(root) = root else {
        return "<?start name=\"doc\"><?end>".into();
    };
    // Render once to learn which unknown namespaces are used, then declare them.
    let body = render_bare(root, names, "");
    let mut decls = format!(" xmlns=\"{}\"", names::SVG_NS);
    for (uri, prefix) in &names.prefixes {
        decls.push_str(&format!(" xmlns:{prefix}=\"{}\"", escape_attr(uri)));
    }
    let tag_end = body.find([' ', '/', '>']).unwrap_or(body.len());
    format!(
        "<?start name=\"doc\">{}{}{}<?end>",
        &body[..tag_end],
        decls,
        &body[tag_end..]
    )
}

/// What a page shows around its region.
pub struct PageInfo<'a> {
    pub doc: &'a str,
    pub snapshot: &'a str,
    pub connected: bool,
    pub seq: u64,
    pub sent_ms: u64,
    pub head: &'a str,
    pub body_end: &'a str,
}

const STYLE: &str = "@view-transition { navigation: auto; }\n\
body { margin: 0; font: 14px system-ui, sans-serif; background: #f4f5f8; color: #11151f; }\n\
header { padding: 8px 16px; display: flex; gap: 12px; }\n\
header code { color: #6b7080; }\n\
main { padding: 0 16px 16px; }\n\
main svg { max-width: 100%; height: auto; display: block; }\n";

/// The header's connection state, in its own marker so it can be patched.
pub fn state_html(connected: bool) -> String {
    let state = if connected {
        "● live"
    } else {
        "○ connecting"
    };
    format!("<?start name=\"state\"><span>{state}</span><?end>")
}

/// A full HTML page around an already-rendered region.
pub fn render_page(region: &str, info: &PageInfo) -> String {
    let short: String = info.snapshot.chars().take(16).collect();
    let state = state_html(info.connected);
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{doc} · XTXP</title><style>{STYLE}</style>{head}</head>\
<body data-seq=\"{seq}\" data-sent=\"{sent}\"><header><b>{doc}</b><code>@{short}</code>{state}</header>\
<main>{region}</main>{body_end}</body></html>\n",
        doc = escape_text(info.doc),
        head = info.head,
        seq = info.seq,
        sent = info.sent_ms,
        body_end = info.body_end,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use xmesh::Xmesh;

    const LOGO: &str = include_str!("../../../xmesh/tests/fixtures/xtxp-logo.svg");

    fn tree(xml: &str) -> Vec<IdNode> {
        let d = Xmesh::new(7).unwrap();
        d.apply(&Xmesh::seed(xml).unwrap()).unwrap();
        d.tree_with_ids()
    }

    fn region(xml: &str) -> String {
        render_region(&tree(xml), &mut TransitionNames::new())
    }

    fn balanced(html: &str) -> bool {
        html.matches("<?start name=").count() == html.matches("<?end>").count()
    }

    #[test]
    fn marker_names_are_node_ids() {
        assert_eq!(
            marker_name(NodeId {
                client: 5,
                clock: 12
            }),
            "n5-12"
        );
    }

    #[test]
    fn logo_renders_as_inline_svg_with_markers() {
        let html = region(LOGO);
        assert!(html.starts_with("<?start name=\"doc\"><svg"), "{html}");
        assert!(html.ends_with("</svg><?end>"), "{html}");
        assert_eq!(
            html.matches("xmlns=\"http://www.w3.org/2000/svg\"").count(),
            1
        );
        assert!(html.matches("<?start name=\"n").count() > 10, "{html}");
        assert!(balanced(&html), "{html}");
        assert!(html.contains("<rect"), "{html}");
        assert!(html.contains("stroke-width=\"9.9\""), "{html}");
    }

    #[test]
    fn transition_names_from_ids() {
        let html = region(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><path id="cross"/><rect/><g id="a"/><g id="a"/><g id="2 bad!"/>t</svg>"#,
        );
        assert!(
            html.contains("style=\"view-transition-name: cross\""),
            "{html}"
        );
        assert!(html.contains("view-transition-name: a\""), "{html}");
        assert!(html.contains("view-transition-name: a-2\""), "{html}");
        assert!(html.contains("view-transition-name: _2-bad-\""), "{html}");
        let rect_name = html.split("<rect").nth(1).unwrap();
        assert!(rect_name.contains("view-transition-name: n0-"), "{html}");
        // The text node `t` carries no style.
        assert!(html.contains("\">t<?end>"), "{html}");
    }

    #[test]
    fn existing_style_is_kept() {
        let html = region(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><rect id="r" style="fill:red"/></svg>"#,
        );
        assert!(
            html.contains("style=\"fill:red; view-transition-name: r\""),
            "{html}"
        );
    }

    #[test]
    fn names_are_stable_across_renders() {
        let t = tree(r#"<svg xmlns="http://www.w3.org/2000/svg"><g id="a"/><g id="a"/></svg>"#);
        let mut names = TransitionNames::new();
        let first = render_region(&t, &mut names);
        let IdNodeKind::Element { children, .. } = &t[0].node else {
            unreachable!()
        };
        let again = render_node(&children[1], &mut names);
        assert!(first.contains("a-2"));
        assert!(again.contains("view-transition-name: a-2"), "{again}");
    }

    #[test]
    fn namespaced_attributes_get_prefixes() {
        let html = region(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" xmlns:x="urn:x"><g inkscape:label="L" x:k="v"/></svg>"#,
        );
        assert!(html.contains("inkscape:label=\"L\""), "{html}");
        assert!(html.contains("ns1:k=\"v\""), "{html}");
        assert!(html.contains("xmlns:ns1=\"urn:x\""), "{html}");
    }

    #[test]
    fn markup_in_values_cannot_break_markers() {
        let html = region(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><g id="q" data-v="a&quot;b&lt;c&amp;d">&lt;?end&gt; &amp; &lt;b&gt;</g><style>.a{} &lt;/style&gt; x</style></svg>"#,
        );
        assert!(html.contains("data-v=\"a&quot;b&lt;c&amp;d\""), "{html}");
        assert!(html.contains("&lt;?end&gt; &amp; &lt;b&gt;"), "{html}");
        assert!(
            html.contains(">.a{} &lt;/style&gt; x<?end></style>"),
            "{html}"
        );
        assert!(balanced(&html), "{html}");
    }

    #[test]
    fn reserved_nodes_render_as_comments() {
        let html = region(r#"<svg xmlns="http://www.w3.org/2000/svg"><!--hello--></svg>"#);
        assert!(html.contains("<!--hello-->"), "{html}");
        assert!(balanced(&html), "{html}");
    }

    #[test]
    fn empty_tree_is_an_empty_region() {
        assert_eq!(
            render_region(&[], &mut TransitionNames::new()),
            "<?start name=\"doc\"><?end>"
        );
    }

    #[test]
    fn page_has_header_and_slots() {
        let page = render_page(
            "<?start name=\"doc\"><svg></svg><?end>",
            &PageInfo {
                doc: "logo",
                snapshot: "abcdef0123456789ff",
                connected: true,
                seq: 7,
                sent_ms: 1000,
                head: "<meta x>",
                body_end: "<script y>",
            },
        );
        let head = &page[..page.find("</head>").unwrap()];
        assert!(head.contains("<meta x>"), "{page}");
        assert!(page.contains("data-seq=\"7\""), "{page}");
        assert!(page.contains("data-sent=\"1000\""), "{page}");
        assert!(page.contains("logo"));
        assert!(page.contains("@abcdef0123456789"));
        assert!(!page.contains("@abcdef0123456789ff"));
        assert!(page.contains("● live"));
        assert!(page.contains("<?start name=\"doc\"><svg></svg><?end>"));
        assert!(page.find("<script y>").unwrap() < page.find("</body>").unwrap());
        let off = render_page(
            "",
            &PageInfo {
                connected: false,
                ..PageInfo {
                    doc: "d",
                    snapshot: "",
                    connected: true,
                    seq: 0,
                    sent_ms: 0,
                    head: "",
                    body_end: "",
                }
            },
        );
        assert!(off.contains("○ connecting"));
    }
}
