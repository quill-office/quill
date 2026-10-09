# Quill

An open-source, cross-platform office suite. Written in Rust, compiled to WebAssembly.
**Quill Write** (word processor) comes first; Sheets, Slides and Mail will share its core.

## Layout

| Crate | Role |
|---|---|
| `quill-core` | Document model and editing operations |
| `quill-layout` | Line breaking, caret positioning, hit testing (measurement is injected) |
| `quill-docx` | `.docx` (OOXML) import/export |
| `quill-wasm` | `wasm-bindgen` bindings (`Editor`) |
| `web/` | Canvas-based editor shell (no framework) |

## Run

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --locked
./build.sh
python3 -m http.server -d web 8080   # open http://localhost:8080
cargo test --workspace
```

## Status: phase 5

Works: typing, selection (mouse drag, Shift+arrows, double/triple-click, Select All), cut/copy/paste,
undo/redo (typing bursts coalesce), bold/italic/underline (also for upcoming typing), Heading 1–3,
bulleted and numbered lists, word wrap with long-word breaking, US Letter pagination, Up/Down/Home/End,
IME composition, tables (insert, Tab between cells, add/delete rows and columns, cells that wrap and
grow their row, rows that move across pages), and `.docx` open/save for all of the above (tabs and
line breaks included), inline images (insert, paste, drag-and-drop, resize, saved into `.docx`),
font family, size and colour per character, paragraph alignment (left, centre, right, justify), and mixed
sizes on one line.

Not yet: floating/wrapped images, drag-resize handles, SVG/EMF images, merged table cells, column resizing, nested tables, highlight/strikethrough/super- and subscript, line and paragraph spacing, indentation, nested lists, headers/footers, page-break control
(widows/orphans), word-wise caret movement (Alt/Ctrl+arrows), find/replace, rich clipboard (HTML),
real-time collaboration, desktop (Tauri) shell. Undo is snapshot-based; it will become an operation log
when collaboration lands.

## License

Apache-2.0
