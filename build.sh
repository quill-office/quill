#!/usr/bin/env bash
# Build the WebAssembly packages: web/pkg (Quill Write) and web/sheets/pkg (Quill Sheets).
# Requires: rustup target wasm32-unknown-unknown, wasm-bindgen-cli.
set -euo pipefail
cd "$(dirname "$0")"
cargo build -p quill-wasm -p quill-sheets-wasm --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir web/pkg target/wasm32-unknown-unknown/release/quill_wasm.wasm
wasm-bindgen --target web --out-dir web/sheets/pkg target/wasm32-unknown-unknown/release/quill_sheets_wasm.wasm
echo "Built web/pkg and web/sheets/pkg — serve with: python3 -m http.server -d web 8080"
