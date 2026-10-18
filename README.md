# xtxp-rs

The Rust side of [XTXP](https://xtxp.link): XML documents (SVG drawings,
first) edited by several people at once, merged as a CRDT on
[Yrs](https://github.com/y-crdt/y-crdt)'s XML types.

This is milestone **M1-y-sweet**: a local *muppet* (an XTXP client) talking
to a stock, unmodified [y-sweet](https://github.com/jamsocket/y-sweet) v0.9.1
over the Yjs sync protocol, to get a feel for a real host before deciding
the full M1 and M2. M1 is command line only. Browsers come in M2.

The spec, the plans and the findings so far are on the
[website](https://xtxp.link) and in
[xtxp-link/website `docs/plans`](https://github.com/xtxp-link/website/tree/main/docs/plans),
starting with `README.md` there.

## Crates

| Path | What it is |
| --- | --- |
| `crates/xmesh` | The pure core: XML documents on Yrs XML types, canonical form, snapshots, node IDs, test vectors. |
| `crates/xtxp-wire` | The peer side of the Yjs sync protocol as y-sweet speaks it, as a pure state machine (plus a fake host for tests). |
| `crates/xtxp-muppet` | The muppet: a terminal UI and one-shot commands. A pure core behind ports, with tokio, WebSocket and ratatui adapters. |
| `tools/chaos-proxy` | A WebSocket proxy that delays and duplicates frames and drops connections, for the end-to-end tests. |
| `tools/fuzzer` | Generates test vectors from a numeric seed. |
| `tools/yrs-compat` | Checks that Xmesh's Yrs 0.28 updates apply the same way in Yrs 0.19.2, which y-sweet runs, and keeps live repros of the findings. |
| `vectors/`, `features/` | Hand-written vectors and the scenarios they check. |

## Try it

On macOS or Linux, with [Rust](https://rustup.rs) installed:

```sh
scripts/try-it.sh
```

It builds y-sweet into `.tools` (once, a few minutes) and the muppet. Then it
starts y-sweet on `localhost:8080` with its data in `.ysweet-data`, seeds the
document `demo` with a blank Inkscape drawing, and prints what to run in two
more terminals:

```sh
target/release/xtxp-muppet open demo
```

Edit in one terminal and watch the change arrive in the other. Ctrl-C stops
y-sweet, and the document stays for next time.

The same steps by hand:

```sh
cargo install --git https://github.com/jamsocket/y-sweet \
  --rev 4f1909b2c3390ced54d32b82a673f4f7ee8ac780 y-sweet --root .tools --locked
cargo build --release -p xtxp-muppet
.tools/bin/y-sweet serve .ysweet-data --port 8080 --skip-gc --checkpoint-freq-seconds 1
# in another terminal:
target/release/xtxp-muppet init crates/xmesh/tests/fixtures/inkscape-blank.svg --doc demo
target/release/xtxp-muppet open demo
```

In `open`:
- **↑ ↓** move;
- **← →** fold and unfold;
- **Enter** edits a text node, or lists an element's attributes: Enter edits one, Backspace or Delete removes it, and the last row, `+ add attribute`, adds one (as does `a`);
- **a** adds an attribute;
- **i** inserts a child, written as XML;
- **d** deletes;
- **m** marks a node to move, then **m** on its new parent;
- **q** quits.

`open --read-only` watches without editing.

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

## Tests

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude xtxp-fuzzer
cargo test --release -p xtxp-fuzzer          # 500 fuzz seeds
cargo test --release -p xtxp-muppet --test e2e
```

The end-to-end tests start real y-sweet servers behind the chaos proxy and
run muppets as separate processes. They need y-sweet in `.tools/bin` (see
above) or at `YSWEET_BIN`. Without it they're skipped with a message, or they
fail when `CI` is set.
