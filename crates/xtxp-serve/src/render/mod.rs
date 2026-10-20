//! The browser renderer: a document's tree as an HTML page with inline SVG,
//! every node wrapped in declarative-partial-update markers, and the patches
//! that replace them. Pure: no network, clock or threads, so it builds for
//! wasm too.

mod assets;
mod dirty;
mod html;
mod page;

pub use assets::{grain_name, metrics_tag, script_tag, LIVE_JS, METRICS_JS, PICKER};
pub use dirty::{template, Grain, PatchSet, Renderer};
pub use html::{render_page, state_html, PageInfo};
pub use page::{Page, Snapshot};
