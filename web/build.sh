#!/bin/sh
# Build the web version: compile gase-web to WebAssembly and put the .wasm
# next to index.html. The page itself needs no build step.
#
#   web/build.sh                 # → web/gase_web.wasm
#   web/build.sh --out DIR       # also copy the whole site to DIR
#                                # (e.g. docs/play for GitHub Pages)
#
# Then serve the folder over HTTP (modules, workers and wasm do not load
# from file://), for example:
#
#   python3 -m http.server -d web 8000     # → http://localhost:8000/
#
# Needs the wasm32-unknown-unknown target:
#   rustup target add wasm32-unknown-unknown
# If binaryen's wasm-opt is installed it is used to shrink the module a
# little more; it is optional.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WEB="$ROOT/web"
OUT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="$2"; shift 2 ;;
        *) echo "usage: $0 [--out DIR]" >&2; exit 2 ;;
    esac
done

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
cargo build --manifest-path "$ROOT/Cargo.toml" -p gase-web \
    --target wasm32-unknown-unknown --profile wasm
WASM="$TARGET_DIR/wasm32-unknown-unknown/wasm/gase_web.wasm"

if command -v wasm-opt >/dev/null 2>&1; then
    # -O3 for speed; the features match what rustc emits by default.
    wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int \
        --enable-sign-ext --enable-mutable-globals "$WASM" -o "$WEB/gase_web.wasm"
else
    cp "$WASM" "$WEB/gase_web.wasm"
fi
echo "web/gase_web.wasm: $(wc -c < "$WEB/gase_web.wasm") bytes"

if [ -n "$OUT" ]; then
    mkdir -p "$OUT/icons"
    for f in index.html gase.css gase.js input.js audio.js audio-worklet.js \
             storage.js sw.js manifest.webmanifest gase_web.wasm; do
        cp "$WEB/$f" "$OUT/$f"
    done
    cp "$WEB"/icons/* "$OUT/icons/"
    echo "site copied to $OUT"
fi
