# Contributing to Quill

Thanks for helping! Quill is early; the best way in is a small, focused change.

## Setup
```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --locked
./build.sh && python3 -m http.server -d web 8080
```

## Before you open a PR
```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Add tests for behaviour changes. The core, layout and docx crates are pure Rust and easy to unit test.

## Layout
`quill-core` (model, editing) → `quill-layout` (pagination, caret) → `quill-docx` (file format) → `quill-wasm` (bindings) → `web/` (UI).
Keep browser concerns out of the Rust crates; text measurement is injected.

By contributing you agree your work is licensed under Apache-2.0.
