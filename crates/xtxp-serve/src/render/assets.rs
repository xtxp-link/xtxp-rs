//! The pages' scripts. Their URLs are relative to the page, so a page works
//! at `/` (`xtxp-serve`) and under a document's path (`xtxp-worker`).

use super::dirty::Grain;

/// The polyfill: applies patches from a WebSocket, or from the stream where
/// `<template for>` isn't native.
pub const LIVE_JS: &str = include_str!("../../assets/live.js");

/// Measures latency and view transitions; `?metrics=1` loads it.
pub const METRICS_JS: &str = include_str!("../../assets/metrics.js");

pub fn grain_name(grain: Grain) -> &'static str {
    match grain {
        Grain::Node => "node",
        Grain::Region => "region",
    }
}

/// The polyfill's tag. Not `defer`: in stream mode the document never
/// finishes parsing, so a deferred script would never run.
pub fn script_tag(mode: &str, grain: Grain) -> String {
    format!(
        "<script src=\"live.js\" data-mode=\"{mode}\" data-grain=\"{}\"></script>",
        grain_name(grain)
    )
}

/// The measurement script's tag, before any other script so it sees every patch.
pub fn metrics_tag(on: bool) -> String {
    if on {
        "<script src=\"metrics.js\"></script>".to_string()
    } else {
        String::new()
    }
}

/// Picks a mode for a browser with JavaScript: the patch stream where
/// `<template for>` works natively, the WebSocket polyfill otherwise.
pub const PICKER: &str = r#"<script>(function () {
  var ok = false;
  try {
    var d = Document.parseHTMLUnsafe('<div id="t"><?start name="t"><?end><template for="t">x</template></div>');
    ok = d.getElementById('t').textContent === 'x';
  } catch (e) {}
  var keep = new URLSearchParams(location.search).get('metrics') === '1' ? '&metrics=1' : '';
  if (ok) { location.replace('?live=stream' + keep); return; }
  var s = document.createElement('script');
  s.src = 'live.js'; s.dataset.mode = 'ws'; s.dataset.grain = 'node';
  document.body.appendChild(s);
})();</script>"#;
