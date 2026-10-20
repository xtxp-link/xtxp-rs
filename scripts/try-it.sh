#!/usr/bin/env bash
# Try XTXP's M1-y-sweet muppet on one machine (macOS or Linux): builds a stock
# y-sweet and the muppet, starts y-sweet on localhost:8080, seeds the document
# `demo`, and tells you what to run in two more terminals.
#
#   --browser  also seeds `logo` (the XTXP logo) and serves it to browsers on
#              localhost:3000, in every delivery mode (M2-y-sweet part A)
#   --reset    first deletes the stored documents, after asking, so they're
#              seeded fresh
#
# Ctrl-C stops everything; the documents stay in .ysweet-data for next time.
set -euo pipefail

YSWEET_REV=4f1909b2c3390ced54d32b82a673f4f7ee8ac780
PORT=8080
WEB_PORT=3000
DOC=demo
LOGO=logo

BROWSER=0
RESET=0
for arg in "$@"; do
    case "$arg" in
        --browser) BROWSER=1 ;;
        --reset) RESET=1 ;;
        *) echo "usage: $0 [--browser] [--reset]" >&2; exit 2 ;;
    esac
done

cd "$(dirname "$0")/.."

listening() {
    lsof -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
}

port_free_or_exit() {
    if listening "$1"; then
        echo "Something is already listening on port $1:" >&2
        lsof -iTCP:"$1" -sTCP:LISTEN >&2 || true
        echo "Stop it (or one left from an earlier run) and try again." >&2
        exit 1
    fi
}

# 0. Reset, if asked.
if [ "$RESET" -eq 1 ]; then
    if listening "$PORT"; then
        echo "y-sweet is running on $PORT; stop it first, since it would write its copy straight back." >&2
        exit 1
    fi
    printf 'Delete .ysweet-data/%s and .ysweet-data/%s? [y/N] ' "$DOC" "$LOGO"
    answer=""
    read -r answer || true
    if [ "$answer" = "y" ] || [ "$answer" = "Y" ]; then
        rm -rf ".ysweet-data/$DOC" ".ysweet-data/$LOGO"
        echo "Deleted; they'll be seeded fresh."
    else
        echo "Kept them."
    fi
fi

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
echo "Building xtxp-muppet and xtxp-tui..."
cargo build --release -p xtxp-muppet -p xtxp-tui
if [ "$BROWSER" -eq 1 ]; then
    cargo build --release -p xtxp-serve
fi

# 3. y-sweet on localhost:8080 (and, with --browser, check 3000 is free too).
port_free_or_exit "$PORT"
if [ "$BROWSER" -eq 1 ]; then
    port_free_or_exit "$WEB_PORT"
fi
mkdir -p .ysweet-data
.tools/bin/y-sweet serve .ysweet-data --port "$PORT" --skip-gc --checkpoint-freq-seconds 1 &
YSWEET_PID=$!
SERVE_PID=""
trap 'kill $YSWEET_PID $SERVE_PID 2>/dev/null || true' EXIT
trap 'echo; echo "Stopping; the documents stay in .ysweet-data."; exit 0' INT TERM

up=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if listening "$PORT"; then
        up=1
        break
    fi
    sleep 0.5
done
if [ "$up" -ne 1 ]; then
    echo "y-sweet didn't start on port $PORT." >&2
    exit 1
fi

# 4. Seed the documents, unless they already have content.
seed() {
    local doc=$1 file=$2 out
    if out=$(target/release/xtxp-muppet --server "localhost:$PORT" init "$file" --doc "$doc" 2>&1); then
        echo "Seeded the document '$doc'."
    elif echo "$out" | grep -q "already has content"; then
        echo "The document '$doc' is already there; keeping it."
    else
        echo "$out" >&2
        exit 1
    fi
}
seed "$DOC" crates/xmesh/tests/fixtures/inkscape-blank.svg
if [ "$BROWSER" -eq 1 ]; then
    seed "$LOGO" crates/xmesh/tests/fixtures/xtxp-logo.svg
    target/release/xtxp-serve "$LOGO" --server "localhost:$PORT" --listen "127.0.0.1:$WEB_PORT" &
    SERVE_PID=$!
fi

# 5. What to run next.
cat <<EOF

y-sweet is running on localhost:$PORT. In two more terminals, run:

    cd $(pwd)
    target/release/xtxp-tui $DOC

Edit in one and watch the change arrive in the other.
EOF
if [ "$BROWSER" -eq 1 ]; then
    cat <<EOF

The XTXP logo is in your browser at:

    http://localhost:$WEB_PORT/                  picks a mode for your browser
    http://localhost:$WEB_PORT/?live=refresh     reloads every second, no JavaScript
    http://localhost:$WEB_PORT/?live=stream      the page's response stays open (Chrome or Edge 150+)
    http://localhost:$WEB_PORT/?live=ws          a WebSocket and a small polyfill

Add &metrics=1 to any of them to measure, and see http://localhost:$WEB_PORT/metrics.
Edit it from a terminal with:

    target/release/xtxp-tui $LOGO

(Try the cross: the third row's first child, stroke-width, with ← and →.)
EOF
fi
cat <<EOF

Press Ctrl-C here to stop.

EOF

# 6. Keep y-sweet in the foreground.
wait "$YSWEET_PID"
