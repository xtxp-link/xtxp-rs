#!/usr/bin/env bash
# Downloads the pinned Chromium trunk snapshot (158, with native
# `<template for>`) into .tools, once, and prints the browser's path.
set -euo pipefail

REV=1722227
cd "$(dirname "$0")/.."
DIR=".tools/chromium-$REV"

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) PLATFORM=Linux_x64; ZIP=chrome-linux.zip; BIN="chrome-linux/chrome" ;;
    Darwin-arm64) PLATFORM=Mac_Arm; ZIP=chrome-mac.zip; BIN="chrome-mac/Chromium.app/Contents/MacOS/Chromium" ;;
    Darwin-x86_64) PLATFORM=Mac; ZIP=chrome-mac.zip; BIN="chrome-mac/Chromium.app/Contents/MacOS/Chromium" ;;
    *) echo "no Chromium snapshot for $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac

if [ ! -x "$DIR/$BIN" ]; then
    mkdir -p "$DIR"
    echo "Downloading Chromium snapshot $REV for $PLATFORM..." >&2
    curl -sSfL -o "$DIR/$ZIP" \
        "https://commondatastorage.googleapis.com/chromium-browser-snapshots/$PLATFORM/$REV/$ZIP"
    (cd "$DIR" && unzip -q -o "$ZIP" && rm "$ZIP")
fi
echo "$PWD/$DIR/$BIN"
