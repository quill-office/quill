//! WebAssembly bindings: an `Editor` that owns a document, selection, and
//! history, and lays itself out through a host-supplied measure callback.
//!
//! The host calls `set_measure(fn(text, bold, italic, size) -> width)`, backed
//! by canvas `measureText`, so layout matches what the browser draws.

use js_sys::Function;
use quill_core::{Align, Attr, Kind, Page, Pos};
use quill_layout::{
    base_size, layout, para_size, Caret, Layout, Measure, PageSetup, Rect, TextStyle,
};
use serde::Serialize;
use wasm_bindgen::prelude::*;

struct JsMeasure<'a>(&'a Function);

impl Measure for JsMeasure<'_> {
    fn width(&self, text: &str, s: &TextStyle) -> f32 {
        let font = s
            .font
            .as_deref()
            .map_or(JsValue::UNDEFINED, JsValue::from_str);
        let args = js_sys::Array::of5(
            &text.into(),
            &s.bold.into(),
            &s.italic.into(),
            &s.size.into(),
            &font,
        );
        self.0
            .apply(&JsValue::NULL, &args)
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32
    }
}

#[derive(Serialize)]
struct State {
    bold: bool,
    italic: bool,
    underline: bool,
    kind: &'static str,
    can_undo: bool,
    can_redo: bool,
    has_selection: bool,
    in_table: bool,
    image_selected: bool,
    /// Effective font size at the caret, in half-points.
    size: u16,
    font: Option<String>,
    color: Option<u32>,
    align: &'static str,
    /// Line spacing in 240ths (0 = default) and left indent in twips.
    line_spacing: u16,
    indent: u16,
    paper: &'static str,
}

#[derive(Serialize)]
struct PageBox {
    w: f32,
    h: f32,
}

#[derive(Serialize)]
struct Frame<'a> {
    layout: &'a Layout,
    caret: Caret,
    selection: Vec<Rect>,
    state: State,
    /// Page size in CSS px.
    page: PageBox,
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Normal => "normal",
        Kind::Heading(1) => "h1",
        Kind::Heading(2) => "h2",
        Kind::Heading(_) => "h3",
        Kind::Bullet => "bullet",
        Kind::Numbered => "numbered",
    }
}

fn parse_kind(s: &str) -> Result<Kind, JsError> {
    Ok(match s {
        "normal" => Kind::Normal,
        "h1" => Kind::Heading(1),
        "h2" => Kind::Heading(2),
        "h3" => Kind::Heading(3),
        "bullet" => Kind::Bullet,
        "numbered" => Kind::Numbered,
        other => return Err(JsError::new(&format!("unknown paragraph kind: {other}"))),
    })
}

#[wasm_bindgen]
pub struct Editor {
    inner: quill_core::Editor,
    measure: Option<Function>,
    /// Horizontal target kept across consecutive Up/Down presses.
    goal_x: Option<f32>,
}

#[wasm_bindgen]
impl Editor {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Editor {
        Editor {
            inner: quill_core::Editor::default(),
            measure: None,
            goal_x: None,
        }
    }

    pub fn set_measure(&mut self, f: Function) {
        self.measure = Some(f);
    }

    /// Paper size: "letter", "a4" or "legal" (margins reset to one inch).
    pub fn set_paper(&mut self, name: &str) -> Result<(), JsError> {
        self.inner.set_page(match name {
            "letter" => Page::LETTER,
            "a4" => Page::A4,
            "legal" => Page::LEGAL,
            other => return Err(JsError::new(&format!("unknown paper size: {other}"))),
        });
        Ok(())
    }

    /// Line spacing in 240ths of a line (240 single, 360 one-and-a-half);
    /// 0 restores the default.
    pub fn set_line_spacing(&mut self, line: u16) {
        self.inner.set_line_spacing((line > 0).then_some(line));
    }

    /// Indent the selected paragraphs by `twips` (negative outdents).
    pub fn indent(&mut self, twips: i32) {
        self.inner.indent(twips);
    }

    fn layout(&self) -> Result<(Layout, Function), JsError> {
        let f = self
            .measure
            .clone()
            .ok_or_else(|| JsError::new("set_measure must be called first"))?;
        let page = PageSetup::from_page(&self.inner.doc().page);
        Ok((layout(self.inner.doc(), page, &JsMeasure(&f)), f))
    }

    // ---- files / text ----------------------------------------------------

    pub fn open_docx(&mut self, bytes: &[u8]) -> Result<(), JsError> {
        let doc = quill_docx::read(bytes).map_err(|e| JsError::new(&e.to_string()))?;
        self.inner.load(doc);
        self.goal_x = None;
        Ok(())
    }

    /// Replace the document with `text`, one paragraph per line. A leading
    /// `# `/`## `/`### ` makes a heading, `- ` a bullet and `N. ` a numbered item.
    pub fn load_text(&mut self, text: &str) {
        let paras = text
            .lines()
            .map(|line| {
                let numbered = line
                    .split_once(". ")
                    .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
                    .map(|(_, r)| (Kind::Numbered, r));
                let (kind, rest) = [
                    ("### ", Kind::Heading(3)),
                    ("## ", Kind::Heading(2)),
                    ("# ", Kind::Heading(1)),
                    ("- ", Kind::Bullet),
                ]
                .into_iter()
                .find_map(|(p, k)| line.strip_prefix(p).map(|r| (k, r)))
                .or(numbered)
                .unwrap_or((Kind::Normal, line));
                quill_core::Paragraph::plain(rest).with_kind(kind)
            })
            .collect();
        self.inner
            .load(quill_core::Document::from_paragraphs(paras));
        self.goal_x = None;
    }

    pub fn save_docx(&self) -> Result<Vec<u8>, JsError> {
        quill_docx::write(self.inner.doc()).map_err(|e| JsError::new(&e.to_string()))
    }

    pub fn text(&self) -> String {
        self.inner.doc().plain_text()
    }

    pub fn selected_text(&self) -> String {
        self.inner.selected_text()
    }

    // ---- editing ---------------------------------------------------------

    pub fn insert_text(&mut self, text: &str) {
        self.goal_x = None;
        self.inner.insert_text(text);
    }

    pub fn enter(&mut self) {
        self.goal_x = None;
        self.inner.enter();
    }

    pub fn backspace(&mut self) {
        self.goal_x = None;
        self.inner.backspace();
    }

    pub fn delete_forward(&mut self) {
        self.goal_x = None;
        self.inner.delete_forward();
    }

    /// Delete the selection and return its text (for the clipboard).
    pub fn cut(&mut self) -> String {
        self.goal_x = None;
        self.inner.cut()
    }

    pub fn undo(&mut self) {
        self.goal_x = None;
        self.inner.undo();
    }

    pub fn redo(&mut self) {
        self.goal_x = None;
        self.inner.redo();
    }

    /// `attr` is "bold", "italic" or "underline".
    pub fn toggle_attr(&mut self, attr: &str) -> Result<(), JsError> {
        let a = match attr {
            "bold" => Attr::Bold,
            "italic" => Attr::Italic,
            "underline" => Attr::Underline,
            other => return Err(JsError::new(&format!("unknown attribute: {other}"))),
        };
        self.inner.toggle_attr(a);
        Ok(())
    }

    /// `kind` is "normal", "h1".."h3", "bullet" or "numbered" (toggles).
    pub fn toggle_kind(&mut self, kind: &str) -> Result<(), JsError> {
        self.inner.toggle_kind(parse_kind(kind)?);
        Ok(())
    }

    /// Font size in half-points (24 = 12 pt); 0 restores the paragraph default.
    pub fn set_size(&mut self, half_points: u16) {
        self.inner
            .set_size((half_points > 0).then_some(half_points));
    }

    /// Font family name; "" restores the default.
    pub fn set_font(&mut self, name: &str) {
        self.inner.set_font(Some(name));
    }

    /// Text colour as 0xRRGGBB; a negative value restores the default.
    pub fn set_color(&mut self, rgb: i32) {
        self.inner.set_color((rgb >= 0).then_some(rgb as u32));
    }

    /// `align` is "left", "center", "right" or "justify".
    pub fn set_align(&mut self, align: &str) -> Result<(), JsError> {
        self.inner.set_align(match align {
            "left" => Align::Left,
            "center" => Align::Center,
            "right" => Align::Right,
            "justify" => Align::Justify,
            other => return Err(JsError::new(&format!("unknown alignment: {other}"))),
        });
        Ok(())
    }

    /// Insert an inline image at the caret. `width`/`height` are the display
    /// size in px. Supported types: PNG, JPEG, GIF, WebP, BMP.
    pub fn insert_image(
        &mut self,
        data: &[u8],
        mime: &str,
        width: u32,
        height: u32,
    ) -> Result<(), JsError> {
        if !matches!(
            mime,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp"
        ) {
            return Err(JsError::new(&format!("unsupported image type: {mime}")));
        }
        self.goal_x = None;
        self.inner.insert_image(mime, data.to_vec(), width, height);
        Ok(())
    }

    /// Scale the selected image(s) by `factor`.
    pub fn scale_images(&mut self, factor: f32) {
        self.inner.scale_selected_images(factor);
    }

    pub fn image_data(&self, id: u32) -> Result<Vec<u8>, JsError> {
        self.inner
            .doc()
            .images
            .get(id as usize)
            .map(|i| i.data.to_vec())
            .ok_or_else(|| JsError::new("no such image"))
    }

    pub fn image_mime(&self, id: u32) -> Result<String, JsError> {
        self.inner
            .doc()
            .images
            .get(id as usize)
            .map(|i| i.mime.clone())
            .ok_or_else(|| JsError::new("no such image"))
    }

    /// Insert a `rows` × `cols` table at the caret (no-op inside a table).
    pub fn insert_table(&mut self, rows: usize, cols: usize) {
        self.goal_x = None;
        self.inner.insert_table(rows, cols);
    }

    /// `op` is "add_row", "add_col", "delete_row", "delete_col" or "delete_table";
    /// it acts on the table containing the caret.
    pub fn table_op(&mut self, op: &str) -> Result<(), JsError> {
        self.goal_x = None;
        match op {
            "add_row" => self.inner.add_row(),
            "add_col" => self.inner.add_col(),
            "delete_row" => self.inner.delete_row(),
            "delete_col" => self.inner.delete_col(),
            "delete_table" => self.inner.delete_table(),
            other => return Err(JsError::new(&format!("unknown table operation: {other}"))),
        }
        Ok(())
    }

    /// Next/previous table cell; types a tab character outside tables.
    pub fn tab(&mut self, backwards: bool) {
        self.goal_x = None;
        self.inner.tab(backwards);
    }

    // ---- cursor ----------------------------------------------------------

    /// Place or extend the caret at a page-local point. `clicks` of 2 selects a
    /// word and 3 a paragraph.
    pub fn click(
        &mut self,
        page: usize,
        x: f32,
        y: f32,
        extend: bool,
        clicks: u32,
    ) -> Result<(), JsError> {
        let (l, f) = self.layout()?;
        let pos = l.hit_test(page, x, y, &JsMeasure(&f));
        self.goal_x = None;
        match clicks {
            2 => self.inner.select_word(pos),
            3.. => self.inner.select_paragraph(pos.para),
            _ => self.inner.set_cursor(pos, extend),
        }
        Ok(())
    }

    pub fn move_left(&mut self, extend: bool) {
        self.goal_x = None;
        self.inner.move_left(extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        self.goal_x = None;
        self.inner.move_right(extend);
    }

    /// Move one visual line up (`dir` < 0) or down.
    pub fn move_vertical(&mut self, dir: i32, extend: bool) -> Result<(), JsError> {
        let (l, f) = self.layout()?;
        let m = JsMeasure(&f);
        let head = self.inner.head();
        let gx = *self.goal_x.get_or_insert_with(|| l.caret(head, &m).x);
        self.inner.set_cursor(l.vertical(head, dir, gx, &m), extend);
        // set_cursor doesn't touch goal_x, so it survives repeated presses.
        Ok(())
    }

    pub fn home(&mut self, extend: bool) -> Result<(), JsError> {
        let (l, _) = self.layout()?;
        let head = self.inner.head();
        self.goal_x = None;
        self.inner.set_cursor(
            Pos {
                para: head.para,
                offset: l.line_range(head).0,
            },
            extend,
        );
        Ok(())
    }

    pub fn end(&mut self, extend: bool) -> Result<(), JsError> {
        let (l, _) = self.layout()?;
        let head = self.inner.head();
        self.goal_x = None;
        self.inner.set_cursor(
            Pos {
                para: head.para,
                offset: l.line_range(head).1,
            },
            extend,
        );
        Ok(())
    }

    pub fn select_all(&mut self) {
        self.goal_x = None;
        self.inner.select_all();
    }

    // ---- rendering -------------------------------------------------------

    /// Lay out the document; returns `{ layout, caret, selection, state }`.
    pub fn frame(&self) -> Result<JsValue, JsError> {
        let (l, f) = self.layout()?;
        let m = JsMeasure(&f);
        let (s, e) = self.inner.selection();
        let fmt = self.inner.current_format();
        let frame = Frame {
            caret: l.caret(self.inner.head(), &m),
            selection: l.selection_rects(s, e, &m),
            state: State {
                bold: fmt.bold,
                italic: fmt.italic,
                underline: fmt.underline,
                kind: kind_name(self.inner.current_kind()),
                can_undo: self.inner.can_undo(),
                can_redo: self.inner.can_redo(),
                has_selection: self.inner.has_selection(),
                in_table: self.inner.in_table(),
                image_selected: self.inner.image_selected(),
                size: fmt.size.unwrap_or_else(|| {
                    let px = para_size(self.inner.current_kind(), base_size(self.inner.doc()));
                    (px * 1.5).round() as u16
                }),
                font: fmt
                    .font
                    .or_else(|| self.inner.doc().defaults.font.clone())
                    .as_deref()
                    .map(str::to_owned),
                color: fmt.color,
                align: match self.inner.current_align() {
                    Align::Left => "left",
                    Align::Center => "center",
                    Align::Right => "right",
                    Align::Justify => "justify",
                },
                line_spacing: self.inner.current_paragraph().line_spacing.unwrap_or(0),
                indent: self.inner.current_paragraph().indent_left.unwrap_or(0),
                paper: match self.inner.doc().page {
                    p if (p.width, p.height) == (Page::LETTER.width, Page::LETTER.height) => {
                        "letter"
                    }
                    p if (p.width, p.height) == (Page::A4.width, Page::A4.height) => "a4",
                    p if (p.width, p.height) == (Page::LEGAL.width, Page::LEGAL.height) => "legal",
                    _ => "custom",
                },
            },
            page: {
                let p = PageSetup::from_page(&self.inner.doc().page);
                PageBox {
                    w: p.width,
                    h: p.height,
                }
            },
            layout: &l,
        };
        serde_wasm_bindgen::to_value(&frame).map_err(|e| JsError::new(&e.to_string()))
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}
