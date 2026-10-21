# xtxp-rs

The Rust side of [XTXP](https://xtxp.link): XML documents (SVG drawings,
first) edited by several people at once, merged as a CRDT on
[Yrs](https://github.com/y-crdt/y-crdt)'s XML types.

This is milestone **M1-y-sweet**: a local *muppet* (an XTXP client) talking
to a stock, unmodified [y-sweet](https://github.com/jamsocket/y-sweet) v0.9.1
over the Yjs sync protocol, to get a feel for a real host before deciding
the full M1 and M2. M1 is command line only. Browsers come in M2.

This repository is the Rust implementation: one muppet and the applications that drive it, each its own binary (`--version` says so). A muppet in another language would be a separate implementation of the same spec, with its own applications.

The spec, the plans and the findings so far are on the
[website](https://xtxp.link) and in
[xtxp-link/website `docs/plans`](https://github.com/xtxp-link/website/tree/main/docs/plans),
starting with `README.md` there.

## Crates

| Path | What it is |
| --- | --- |
| `crates/xmesh` | The pure core: XML documents on Yrs XML types, canonical form, snapshots, node IDs, test vectors. |
| `crates/xtxp-wire` | The peer side of the Yjs sync protocol as y-sweet speaks it, as a pure state machine (plus a fake host for tests). |
| `crates/xtxp-muppet` | The muppet executor: Xmesh embedding, sync, tree, and the one-shot commands (`init`, `edit`, `show`, `replay`). Pure core behind ports, with tokio and WebSocket adapters. |
| `crates/xtxp-serve` | A browser application on a muppet: serves rendered SVG documents to browsers with live updates in three modes. |
| `crates/xtxp-tui` | A terminal UI application on a muppet: edits documents interactively with keyboard commands. |
| `crates/xtxp-worker` | A host and browser server on a Cloudflare Worker, compiled to wasm: one Durable Object per document speaks y-sweet's protocol to muppets and serves `xtxp-serve`'s pages to browsers. Its own workspace. |
| `crates/xtxp-orbit` | An application that owns a space: a three-body physics engine that moves and spins the logo's parts, publishing their transforms through a muppet. |
| `tools/chaos-proxy` | A WebSocket proxy that delays and duplicates frames and drops connections, for the end-to-end tests. |
| `tools/bench-browser` | Load for `serve`: viewers in each delivery mode and an editor on y-sweet, measuring latency and bandwidth. |
| `tests/browser` | Playwright tests of `serve` in Chromium 158 (native `<template for>`) and 141 (without). |
| `tools/fuzzer` | Generates test vectors from a numeric seed. |
| `tools/yrs-compat` | Checks that Xmesh's Yrs 0.28 updates apply the same way in Yrs 0.19.2, which y-sweet runs, and keeps live repros of the findings. |
| `vectors/`, `features/` | Hand-written vectors and the scenarios they check. |

## Try it

On macOS or Linux, with [Rust](https://rustup.rs) installed:

```sh
scripts/try-it.sh
```

It builds y-sweet into `.tools` (once, a few minutes) and the muppet and apps. Then it
starts y-sweet on `localhost:8080` with its data in `.ysweet-data`, seeds the
document `demo` with a blank Inkscape drawing, and prints what to run in two
more terminals:

```sh
target/release/xtxp-tui demo
```

Edit in one terminal and watch the change arrive in the other. Ctrl-C stops
y-sweet, and the document stays for next time.

The same steps by hand:

```sh
cargo install --git https://github.com/jamsocket/y-sweet \
  --rev 4f1909b2c3390ced54d32b82a673f4f7ee8ac780 y-sweet --root .tools --locked
cargo build --release -p xtxp-muppet -p xtxp-tui
.tools/bin/y-sweet serve .ysweet-data --port 8080 --skip-gc --checkpoint-freq-seconds 1
# in another terminal:
target/release/xtxp-muppet init crates/xmesh/tests/fixtures/inkscape-blank.svg --doc demo
target/release/xtxp-tui demo
```

In `xtxp-tui`:
- **↑ ↓** move;
- **← →** fold and unfold;
- **Enter** edits a text node, or lists an element's attributes: Enter edits one, **← →** nudge a numeric one by its last digit (Shift for ×10), Backspace or Delete removes one, and the last row, `+ add attribute`, adds one (as does `a`);
- **a** adds an attribute;
- **i** inserts a child, written as XML;
- **d** deletes;
- **m** marks a node to move, then **m** on its new parent;
- **q** or **Ctrl-C** quits.

`xtxp-tui --read-only` watches without editing.

The one-shot commands print JSON or XML and exit, for scripts and tests:

```sh
xtxp-muppet show demo --tree        # paths, for the commands below
xtxp-muppet show demo --canonical   # canonical XML
xtxp-muppet show demo --hashes
xtxp-muppet edit demo set-attr 5/1 fill red , remove-attr 5/1 id
```

The ops are `set-attr`, `remove-attr`, `insert`, `insert-text`, `remove` and
`move`. Separate several with `,`; they apply as one batch. `--server` picks
another y-sweet (default `localhost:8080`).

## In a browser

M2-y-sweet's first part shows a document live in a browser, watch-only for now:

```sh
scripts/try-it.sh --browser
```

It also seeds the XTXP logo as `logo` and runs `xtxp-serve logo` on `localhost:3000`. Edit the logo with `target/release/xtxp-tui logo` and watch the browser. The same drawing comes in three ways:

| URL | How changes arrive | JavaScript |
| --- | --- | --- |
| `/?live=refresh` | The page reloads every second; unchanged polls get `304 Not Modified`. | none |
| `/?live=stream` | The page's response stays open and each change arrives as a `<template for>` patch ([declarative partial updates](https://xtxp.link/spec/browser/)). Needs Chrome or Edge 150 or later. | none |
| `/?live=ws` | A WebSocket and a small polyfill (`live.js`) that applies the same patches in any modern browser. | `live.js` |

`/` with no parameters picks one: the stream where the browser supports it, the WebSocket otherwise, and the refresh with JavaScript off. `&grain=node` or `&grain=region` chooses whether a patch replaces just the changed elements or the whole drawing.

To measure, add `&metrics=1` (latency and view-transition time in an overlay, sent to the server) and read `http://localhost:3000/metrics`. `tools/bench-browser` puts load on it: N viewers in one mode and an editor on y-sweet, reporting latency, bandwidth and server CPU. `scripts/bench-browser.sh` runs every mode at 1 to 1,000 viewers. What the browsers did is in [`docs/reports/m2-y-sweet-browser-probes.md`](docs/reports/m2-y-sweet-browser-probes.md).

`--reset` deletes the stored documents first (after asking), so they're seeded fresh.

## Physics

With `scripts/try-it.sh --browser` running, `xtxp-orbit` animates the logo:

```sh
cargo build --release -p xtxp-orbit
target/release/xtxp-orbit logo
```

The cross is a heavy body at its place in the logo. The two outer X's start on a circular orbit about it, one moving up and one down, and spin about their own centres. The physics steps at a fixed 240 Hz and the transforms are published at 30 Hz (`--physics-hz`, `--render-hz`). `--help` lists the masses, gravity, start positions and velocities, and `--left-spin`, `--cross-spin` and `--right-spin` (degrees per second). Ctrl-C leaves the logo where it stopped; `scripts/try-it.sh --reset --browser` puts it back.

## On a Cloudflare Worker

`crates/xtxp-worker` is the host and the browser server in one, compiled to
wasm for a Cloudflare Worker. Each document is a Durable Object that speaks
y-sweet's protocol, so `xtxp-tui`, `xtxp-orbit` and `xtxp-muppet` connect
to it unchanged, and renders the document for any number of browsers in the
same three modes as `xtxp-serve`, under `/d/{doc}/`. To run it locally, in
Cloudflare's own runtime and with no account:

```sh
scripts/try-worker.sh
```

It needs Node 22 or later as well as Rust. It seeds the logo and prints the
URLs; drive it with `--server 127.0.0.1:8787`.

To put it on the Workers free plan, from `crates/xtxp-worker`:

```sh
npx wrangler login
npx wrangler deploy        # prints https://xtxp-worker.<your-subdomain>.workers.dev
```

then point the muppets at it with `--server https://xtxp-worker.<your-subdomain>.workers.dev`
and open `https://xtxp-worker.<your-subdomain>.workers.dev/d/logo/` in browsers.

What the free plan means for it:

- A document stays in memory while anyone is connected and is written to
  storage at most every 2 seconds, so a muppet editing at 30 Hz costs about
  one row write per 2 s, not 30 a second.
- Its WebSockets aren't hibernatable yet, so a document with someone connected
  counts duration the whole time: roughly a day of one document's connected
  time fits in the free daily allowance.
- Muppet frames count as requests, 20 to one: `xtxp-orbit` at its default
  30 Hz uses about 5,400 of the 100,000 daily requests an hour.
  `--render-hz 10` stretches that. Browsers on `?live=refresh` cost a request
  a second each; streams and WebSocket viewers cost one when they open.
- There's no auth, as with y-sweet here (P2): anyone with the URL can edit.
- `?metrics=1` isn't served yet.

## Tests

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude xtxp-fuzzer --exclude bench-browser
cargo test --release -p xtxp-fuzzer          # 500 fuzz seeds
cargo test --release -p xtxp-muppet --test e2e
cargo test --release -p bench-browser          # a smoke run of every mode
scripts/fetch-chromium.sh                     # Chromium 158, once
(cd tests/browser && npm ci && npx playwright test)
(cd crates/xtxp-worker && npm ci && npx wrangler dev) &   # then:
node tests/worker/smoke.mjs
```

The end-to-end tests start real y-sweet servers behind the chaos proxy and
run muppets as separate processes. They need y-sweet in `.tools/bin` (see
above) or at `YSWEET_BIN`. Without it they're skipped with a message, or they
fail when `CI` is set.
