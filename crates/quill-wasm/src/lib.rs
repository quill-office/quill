//! WebAssembly bindings: an `Editor` that owns a document, selection, and
//! history, and lays itself out through a host-supplied measure callback.
//!
//! The host calls `set_measure(fn(text, bold, italic, size) -> width)`, backed
//! by canvas `measureText`, so layout matches what the browser draws.

use js_sys::Function;
use quill_core::{Attr, Kind, Pos};
use quill_layout::{layout, Caret, Layout, Measure, PageSetup, Rect, TextStyle};
use serde::Serialize;
use wasm_bindgen::prelude::*;

struct JsMeasure<'a>(&'a Function);

impl Measure for JsMeasure<'_> {
    fn width(&self, text: &str, s: &TextStyle) -> f32 {
        self.0
            .call4(
                &JsValue::NULL,
                &text.into(),
                &s.bold.into(),
                &s.italic.into(),
                &s.size.into(),
            )
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
}

#[derive(Serialize)]
struct Frame<'a> {
    layout: &'a Layout,
    caret: Caret,
    selection: Vec<Rect>,
    state: State,
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
    page: PageSetup,
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
            page: PageSetup::LETTER,
            measure: None,
            goal_x: None,
        }
    }

    pub fn set_measure(&mut self, f: Function) {
        self.measure = Some(f);
    }

    pub fn set_page(&mut self, width: f32, height: f32, margin: f32) {
        self.page = PageSetup {
            width,
            height,
            margin,
        };
    }

    fn layout(&self) -> Result<(Layout, Function), JsError> {
        let f = self
            .measure
            .clone()
            .ok_or_else(|| JsError::new("set_measure must be called first"))?;
        Ok((layout(self.inner.doc(), self.page, &JsMeasure(&f)), f))
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
