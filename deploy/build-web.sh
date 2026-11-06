#!/usr/bin/env bash
# Builds the browser client into dist/dronoids/: the page, macroquad's JS
# loader, our plugin (web/god_net.js) and the wasm module.
#
#   ./deploy/build-web.sh            release build
#   ./deploy/build-web.sh --serve    ...then serve it on http://127.0.0.1:8090
set -euo pipefail
cd "$(dirname "$0")/.."

out=dist/dronoids
cargo build --locked --release --target wasm32-unknown-unknown --bin dronoids

# macroquad's loader ships inside the crate: take the one matching Cargo.lock.
version=$(grep -A1 '^name = "macroquad"$' Cargo.lock | sed -n 's/^version = "\(.*\)"$/\1/p')
bundle=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/macroquad-"$version"/js/mq_js_bundle.js | head -n1)

rm -rf "$out"
mkdir -p "$out"
cp web/index.html web/god_net.js "$out/"
cp "$bundle" "$out/mq_js_bundle.js"
cp target/wasm32-unknown-unknown/release/dronoids.wasm "$out/"
if command -v wasm-opt >/dev/null; then
    wasm-opt -Os "$out/dronoids.wasm" -o "$out/dronoids.wasm"
fi
ls -la "$out"

if [ "${1:-}" = "--serve" ]; then
    python -m http.server 8090 --bind 127.0.0.1 --directory "$out"
fi
