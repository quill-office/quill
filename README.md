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
| `desktop/` | Tauri 2 desktop app that wraps `web/` with native file dialogs |

## Run

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --locked
./build.sh
python3 -m http.server -d web 8080   # open http://localhost:8080
cargo test --workspace
```

## Desktop app

`desktop/` wraps the web editor in a native window (Tauri 2) with real Open / Save / Save As / PDF dialogs,
an unsaved-changes marker in the title bar, and a confirmation before closing with unsaved changes. The page never
sees file paths: Rust commands show the dialog and read or write only the file the user picked.

```sh
cd desktop
npm install
npm run dev      # builds the WASM package, then opens the app
npm run build    # produces an installable bundle in desktop/src-tauri/target/release/bundle
```

Requires Rust, Node 18+, and the platform's Tauri prerequisites (Xcode command-line tools on macOS).
The desktop crate is not part of the Cargo workspace, so `cargo test --workspace` doesn't need Tauri.

### Releases

Pushing a tag such as `v0.1.0` runs `.github/workflows/release.yml`, which builds installers for macOS (Apple
silicon and Intel), Windows and Linux and attaches them to a **draft** GitHub release for you to review and publish.
The builds are **unsigned**: macOS Gatekeeper and Windows SmartScreen will warn on first launch (on macOS,
right-click → Open). Signing needs an Apple Developer ID / a Windows certificate and repository secrets; it isn't
set up yet.

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
real-time collaboration, file associations and "open with" from the OS, auto-update, code signing, mobile. Undo is snapshot-based; it will become an operation log
when collaboration lands.

## License

Apache-2.0
