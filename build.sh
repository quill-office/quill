#!/usr/bin/env bash
# Build the WASM package into web/pkg. Requires: rustup target wasm32-unknown-unknown, wasm-bindgen-cli.
set -euo pipefail
cd "$(dirname "$0")"
cargo build -p quill-wasm --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir web/pkg target/wasm32-unknown-unknown/release/quill_wasm.wasm
echo "Built web/pkg — serve with: python3 -m http.server -d web 8080"
