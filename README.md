# Quill

An open-source, cross-platform office suite. Written in Rust, compiled to WebAssembly.
**Quill Write** (word processor) comes first; Sheets, Slides and Mail will share its core.

## Layout

| Crate | Role |
|---|---|
| `quill-core` | Document model and editing operations |
| `quill-layout` | Line breaking, caret positioning, hit testing (measurement is injected) |
| `quill-docx` | `.docx` (OOXML) import/export |
| `quill-pdf` | PDF writer driven by the layout (standard fonts, embedded images) |
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

## Status: phase 9

Works: typing, selection (mouse drag, Shift+arrows, double/triple-click, Select All), cut/copy/paste,
undo/redo (typing bursts coalesce), bold/italic/underline (also for upcoming typing), Heading 1–3,
bulleted and numbered lists, word wrap with long-word breaking, US Letter pagination, Up/Down/Home/End,
IME composition, tables (insert, Tab between cells, add/delete rows and columns, cells that wrap and
grow their row, rows that move across pages), and `.docx` open/save for all of the above (tabs and
line breaks included), inline images (insert, paste, drag-and-drop, resize, saved into `.docx`),
font family, size and colour per character, paragraph alignment (left, centre, right, justify), and mixed
sizes on one line; paragraph spacing, line spacing and indents; paper sizes (Letter, A4, Legal) and page margins; headers and footers with page-number and page-count
fields (double-click the top or bottom margin, or use the Page no. button), saved to and read from `.docx`; find and replace (case and whole-word options, live highlighting,
replace all as one undo step); PDF export.

`.docx` import resolves Word's styling: document defaults, theme fonts, paragraph and character styles with
`basedOn` inheritance, table-style paragraph properties, and the section's page size and margins.

PDF export lays the document out again with Arial / Times New Roman / Courier New (metric-compatible with the
PDF standard fonts Helvetica / Times / Courier), so nothing needs embedding but line breaks can differ slightly from
the screen, and only Western-European (WinAnsi) characters are supported; others print as `?`. No links,
bookmarks or tagged (accessible) structure yet.

Not yet: regular-expression and formatting search, searching all stories at once, floating/wrapped images, drag-resize handles, SVG/EMF images, merged table cells, column resizing, nested tables, highlight/strikethrough/super- and subscript, nested lists, conditional table formatting (banded rows etc.),
multiple sections, different first-page and odd/even headers, pictures and tables in headers/footers,
other fields (date, title, …), exact/at-least line spacing, right indents, headers/footers, page-break control
(widows/orphans), word-wise caret movement (Alt/Ctrl+arrows), find/replace, rich clipboard (HTML),
real-time collaboration, desktop (Tauri) shell. Undo is snapshot-based; it will become an operation log
when collaboration lands.

## License

Apache-2.0
