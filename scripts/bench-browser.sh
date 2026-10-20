#!/usr/bin/env bash
# Runs bench-browser for every mode and grain at several viewer counts,
# against a running `xtxp-muppet serve logo` and its y-sweet (as
# `scripts/try-it.sh --browser` starts them). Writes bench-results.csv and
# bench-results.md. Raise `ulimit -n` first for 1,000 viewers.
set -euo pipefail

cd "$(dirname "$0")/.."
URL=${URL:-http://127.0.0.1:3000}
YSWEET=${YSWEET:-localhost:8080}
VIEWERS=${VIEWERS:-"1 10 100 1000"}
RATE=${RATE:-5}
SECONDS_EACH=${SECONDS_EACH:-30}

cargo build --release -q -p bench-browser
BIN=target/release/bench-browser
rm -f bench-results.csv
for n in $VIEWERS; do
    for run in "refresh region" "stream region" "stream node" "ws node" "ws region"; do
        read -r mode grain <<<"$run"
        echo "== $mode/$grain, $n viewers" >&2
        "$BIN" --url "$URL" --ysweet "$YSWEET" --mode "$mode" --grain "$grain" --viewers "$n" \
            --edits-per-sec "$RATE" --seconds "$SECONDS_EACH" --out bench-results.csv >/dev/null \
            || echo "   failed" >&2
    done
done
"$BIN" --table bench-results.csv | tee bench-results.md
