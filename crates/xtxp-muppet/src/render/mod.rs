//! The browser renderer: a document's tree as an HTML page with inline SVG,
//! every node wrapped in declarative-partial-update markers, and the patches
//! that replace them. Pure: no network, clock or threads.

mod dirty;
mod html;

pub use dirty::{template, Grain, PatchSet, Renderer};

pub use html::{
    marker_name, render_node, render_page, render_region, state_html, PageInfo, TransitionNames,
};
