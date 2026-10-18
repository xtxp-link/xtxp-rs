//! Namespaces Xmesh knows by name, and expanded names (`{uri}local`).

pub const XTXP_NS: &str = "https://xtxp.link/ns";
pub const SVG_NS: &str = "http://www.w3.org/2000/svg";
pub const INKSCAPE_NS: &str = "http://www.inkscape.org/namespaces/inkscape";
pub const SODIPODI_NS: &str = "http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd";
pub const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// Reserved element holding a comment's text.
pub const COMMENT: &str = "{https://xtxp.link/ns}comment";
/// Reserved element for a processing instruction: target in `target`, data as text.
pub const PI: &str = "{https://xtxp.link/ns}pi";
/// Reserved element holding a CDATA section's text.
pub const CDATA: &str = "{https://xtxp.link/ns}cdata";

/// Prefixes with a fixed meaning, used when parsing fragments and writing canonical XML.
pub const FIXED_PREFIXES: [(&str, &str); 4] = [
    ("inkscape", INKSCAPE_NS),
    ("sodipodi", SODIPODI_NS),
    ("xlink", XLINK_NS),
    ("xml", XML_NS),
];

/// Builds an expanded name: `{uri}local`, or `local` with no namespace.
pub fn expand(uri: Option<&str>, local: &str) -> String {
    match uri {
        Some(uri) => format!("{{{uri}}}{local}"),
        None => local.to_string(),
    }
}

/// Splits an expanded name into its namespace URI and local name.
pub fn split(expanded: &str) -> (Option<&str>, &str) {
    match expanded
        .strip_prefix('{')
        .and_then(|rest| rest.split_once('}'))
    {
        Some((uri, local)) => (Some(uri), local),
        None => (None, expanded),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_and_split_round_trip() {
        assert_eq!(
            expand(Some(SVG_NS), "rect"),
            "{http://www.w3.org/2000/svg}rect"
        );
        assert_eq!(
            split("{http://www.w3.org/2000/svg}rect"),
            (Some(SVG_NS), "rect")
        );
        assert_eq!(expand(None, "foo"), "foo");
        assert_eq!(split("foo"), (None, "foo"));
    }
}
