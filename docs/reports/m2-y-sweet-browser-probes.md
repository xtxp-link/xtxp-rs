# M2-y-sweet part A: automated browser probes

What `tests/browser/probes.spec.mjs` found, run on 2026-10-19 in this repository's CI container. The raw output is written to `tests/browser/test-results/probes-<browser>.json`.

| | Chromium 158.0.8113.0 (trunk snapshot 1722227) | Chromium 141.0.7390.37 (Playwright 1.56.1) |
| --- | --- | --- |
| Native `<template for>`, detected with `Document.parseHTMLUnsafe` | yes | no |
| `Element.prototype.streamHTML` | present | absent |
| Native stream, **node** grain: namespace of a patched SVG element | `http://www.w3.org/1999/xhtml`: an HTML element, which never draws | n/a |
| Does a streamed `<template for>` patch start a view transition by itself? | **no**: no view-transition animations ran | n/a |
| WebSocket mode, node grain, inside `startViewTransition()`: transition groups that animated | `root` and the `<svg>` element's (`n0-0`) only | the same |
| Does an SVG child (the cross, `view-transition-name: n0-255`) get its own group? | **no** | **no** |

## What it means

1. **Native patches can't target SVG elements.** `<template for>` content is parsed in an HTML context, so per-node patches inside inline SVG produce HTML elements. Native stream mode therefore patches the whole `<svg>` region (its default), and per-node SVG patches need contextual parsing, which `live.js` does.
2. **A streamed patch is not animated.** With no script, a `<template for>` patch replaces the content instantly. Animating it needs the one `document.startViewTransition()` call, which needs script; no-JS streaming is live but not animated.
3. **SVG children don't take part in view transitions.** `view-transition-name` on elements inside an `<svg>` is ignored: only boxes in the CSS layout (the `<svg>` element itself) get groups. A change inside the drawing animates as a cross-fade of the whole drawing. Per-element animation needs an HTML layer over the drawing, or SVG/SMIL or CSS animations of its own.
4. **Detecting `<template for>`** works with `Document.parseHTMLUnsafe` and a marker pair, as the bare page's picker does.

## A flake in headless Chromium

About 1 run in 20, in both versions, a page with JavaScript off on the refresh loop receives a reload's `200` with the new document and then never shows it: the navigation isn't committed and the meta refresh stops. Logs from `serve` (the request, its `If-None-Match`, the `ETag` and the status) and from the test (each response's body) showed the server's answer was correct. Removing the cross-document view transition rule didn't stop it. The browser tests allow one retry for it; whether a real, visible Chrome ever does this is on the cross-test checklist.
