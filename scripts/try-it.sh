#!/usr/bin/env bash
# Try XTXP's M1-y-sweet muppet on one machine (macOS or Linux): builds a stock
# y-sweet and the muppet, starts y-sweet on localhost:8080, seeds the document
# `demo`, and tells you what to run in two more terminals.
# Ctrl-C stops y-sweet; the document stays in .ysweet-data for next time.
set -euo pipefail

YSWEET_REV=4f1909b2c3390ced54d32b82a673f4f7ee8ac780
PORT=8080
DOC=demo

cd "$(dirname "$0")/.."

# 1. Rust.
if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo isn't installed. Install Rust from https://rustup.rs:" >&2
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
    echo "then open a new terminal and run this script again." >&2
    exit 1
fi

# 2. y-sweet (once, a few minutes) and the muppet.
if [ ! -x .tools/bin/y-sweet ]; then
    echo "Building y-sweet v0.9.1 into .tools (once; a few minutes)..."
    cargo install --git https://github.com/jamsocket/y-sweet --rev "$YSWEET_REV" \
        y-sweet --root .tools --locked
fi
echo "Building xtxp-muppet..."
cargo build --release -p xtxp-muppet

# 3. y-sweet on localhost:8080.
if lsof -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "Something is already listening on port $PORT:" >&2
    lsof -iTCP:"$PORT" -sTCP:LISTEN >&2 || true
    echo "Stop it (or a y-sweet left from an earlier run) and try again." >&2
    exit 1
fi
mkdir -p .ysweet-data
.tools/bin/y-sweet serve .ysweet-data --port "$PORT" --skip-gc --checkpoint-freq-seconds 1 &
YSWEET_PID=$!
trap 'kill "$YSWEET_PID" 2>/dev/null || true' EXIT
trap 'echo; echo "Stopping y-sweet; the document stays in .ysweet-data."; exit 0' INT TERM

# 4. Seed the document, unless it already has content.
up=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if lsof -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
        up=1
        break
    fi
    sleep 0.5
done
if [ "$up" -ne 1 ]; then
    echo "y-sweet didn't start on port $PORT." >&2
    exit 1
fi
if out=$(target/release/xtxp-muppet --server "localhost:$PORT" init \
    crates/xmesh/tests/fixtures/inkscape-blank.svg --doc "$DOC" 2>&1); then
    echo "Seeded the document '$DOC'."
elif echo "$out" | grep -q "already has content"; then
    echo "The document '$DOC' is already there; keeping it."
else
    echo "$out" >&2
    exit 1
fi

# 5. What to run next.
cat <<EOF

y-sweet is running on localhost:$PORT. In two more terminals, run:

    cd $(pwd)
    target/release/xtxp-muppet open $DOC

Edit in one and watch the change arrive in the other.
Press Ctrl-C here to stop y-sweet.

EOF

# 6. Keep y-sweet in the foreground.
wait "$YSWEET_PID"
