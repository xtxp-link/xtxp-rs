#!/usr/bin/env bash
# Try xtxp-worker on one machine (macOS or Linux): builds the Worker and the
# muppets, runs the Worker under `wrangler dev` on localhost:8787 (Cloudflare's
# own runtime, locally; nothing is deployed and no account is needed), seeds
# the XTXP logo as `logo`, and tells you what to run next.
#
# Ctrl-C stops it; the documents stay in crates/xtxp-worker/.wrangler for next time.
set -euo pipefail

PORT=8787
DOC=logo

cd "$(dirname "$0")/.."

for tool in cargo rustup npm; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "$tool isn't installed: Rust from https://rustup.rs, npm with Node 22 or later." >&2
        exit 1
    fi
done
if lsof -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "Something is already listening on port $PORT; stop it and try again." >&2
    exit 1
fi

echo "Building the muppets..."
cargo build --release -p xtxp-muppet -p xtxp-tui -p xtxp-orbit
rustup target add wasm32-unknown-unknown
(cd crates/xtxp-worker && npm install --no-fund --no-audit)

# wrangler builds the Worker (worker-build, once a few minutes) and serves it.
(cd crates/xtxp-worker && exec npx wrangler dev --port "$PORT" --ip 127.0.0.1) &
WRANGLER_PID=$!
trap 'kill $WRANGLER_PID 2>/dev/null || true' EXIT
trap 'echo; echo "Stopping; the documents stay in crates/xtxp-worker/.wrangler."; exit 0' INT TERM

echo "Waiting for the Worker (the first build takes a few minutes)..."
until curl -sf "http://127.0.0.1:$PORT/" >/dev/null; do
    if ! kill -0 "$WRANGLER_PID" 2>/dev/null; then
        echo "wrangler stopped; see its output above." >&2
        exit 1
    fi
    sleep 1
done

SERVER="127.0.0.1:$PORT"
if out=$(target/release/xtxp-muppet --server "$SERVER" init crates/xmesh/tests/fixtures/xtxp-logo.svg --doc "$DOC" 2>&1); then
    echo "Seeded the document '$DOC'."
elif echo "$out" | grep -q "already has content"; then
    echo "The document '$DOC' is already there; keeping it."
else
    echo "$out" >&2
    exit 1
fi

cat <<EOF

The Worker is running on localhost:$PORT: the host and the browser server in one.
Open the logo in as many browsers as you like:

    http://localhost:$PORT/d/$DOC/                  picks a mode for your browser
    http://localhost:$PORT/d/$DOC/?live=refresh     reloads every second, no JavaScript
    http://localhost:$PORT/d/$DOC/?live=stream      the page's response stays open (Chrome or Edge 150+)
    http://localhost:$PORT/d/$DOC/?live=ws          a WebSocket and a small polyfill

Drive it from more terminals:

    target/release/xtxp-tui $DOC --server $SERVER
    target/release/xtxp-orbit $DOC --server $SERVER

Press Ctrl-C here to stop.

EOF
wait "$WRANGLER_PID"
