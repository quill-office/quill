//! WebAssembly bindings: an `Editor` that owns a document, selection, and
//! history, and lays itself out through a host-supplied measure callback.
//!
//! The host calls `set_measure(fn(text, bold, italic, size) -> width)`, backed
//! by canvas `measureText`, so layout matches what the browser draws.

use std::collections::HashMap;

use js_sys::Function;
use quill_core::{
    Align, Attr, FindDir, FindOptions, Kind, Page, Pos, Story, PAGES_CHAR, PAGE_CHAR,
};
use quill_layout::{
    base_size, layout, para_size, Caret, Focus, Layout, Measure, PageSetup, Rect, TextStyle,
};
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// Measures with the metric-compatible families the PDF's standard fonts
/// match, so layout widths equal the widths in the PDF.
struct PdfMeasure<'a>(&'a Function);

impl Measure for PdfMeasure<'_> {
    fn width(&self, text: &str, s: &TextStyle) -> f32 {
        let family = quill_pdf::Family::of(s.font.as_deref());
        JsMeasure(self.0).width(
            text,
            &TextStyle {
                font: Some(family.css().into()),
                ..s.clone()
            },
        )
    }
}

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
    /// Which story is being edited: "body", "header" or "footer".
    story: &'static str,
}

#[derive(Serialize)]
struct PageBox {
    w: f32,
    h: f32,
    /// Top and bottom margins, where headers and footers live.
    top: f32,
    bottom: f32,
}

/// Search state for drawing: where every match is, and which one is selected.
#[derive(Serialize)]
struct FindInfo {
    rects: Vec<Rect>,
    count: usize,
    index: Option<usize>,
}

#[derive(Serialize)]
struct Frame<'a> {
    layout: &'a Layout,
    caret: Caret,
    selection: Vec<Rect>,
    state: State,
    /// Page size in CSS px.
    page: PageBox,
    find: FindInfo,
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
    /// Page where the header/footer caret is shown (they repeat on every page).
    focus_page: usize,
    /// The active search, if any.
    find: Option<(String, FindOptions)>,
    /// Decoded images supplied for the next PDF export.
    pdf_images: HashMap<u32, quill_pdf::PdfImage>,
}

#[wasm_bindgen]
impl Editor {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Editor {
        Editor {
            inner: quill_core::Editor::default(),
            measure: None,
            goal_x: None,
            focus_page: 0,
            find: None,
            pdf_images: HashMap::new(),
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
        let mut l = layout(self.inner.doc(), page, &JsMeasure(&f));
        l.focus = Focus {
            story: self.inner.story(),
            page: self.focus_page,
        };
        Ok((l, f))
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

    /// Edit the "body", "header" or "footer". Headers and footers repeat on
    /// every page, so `page` says where the caret is drawn.
    pub fn enter_story(&mut self, story: &str, page: usize) -> Result<(), JsError> {
        let story = match story {
            "body" => Story::Body,
            "header" => Story::Header,
            "footer" => Story::Footer,
            other => return Err(JsError::new(&format!("unknown story: {other}"))),
        };
        self.goal_x = None;
        self.focus_page = page;
        self.inner.enter_story(story);
        Ok(())
    }

    /// Insert a "page" or "pages" (page count) field; headers and footers only.
    pub fn insert_field(&mut self, kind: &str) -> Result<(), JsError> {
        self.goal_x = None;
        self.inner.insert_field(match kind {
            "page" => PAGE_CHAR,
            "pages" => PAGES_CHAR,
            other => return Err(JsError::new(&format!("unknown field: {other}"))),
        });
        Ok(())
    }

    /// Give the document a centred page-number footer.
    pub fn add_page_numbers(&mut self) {
        self.inner.add_page_numbers();
    }

    // ---- PDF export ----------------------------------------------------------

    /// Ids of the body's images. Decode each in the host and pass its pixels
    /// to `set_pdf_image` before calling `export_pdf`.
    pub fn pdf_image_ids(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        for p in self.inner.doc().story_paras(Story::Body) {
            for id in p.runs.iter().filter_map(|r| r.format.image) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    /// Supply decoded 8-bit RGBA pixels for an image.
    pub fn set_pdf_image(
        &mut self,
        id: u32,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<(), JsError> {
        if (width as usize) * (height as usize) * 4 != rgba.len() {
            return Err(JsError::new("pixel data does not match the image size"));
        }
        self.pdf_images.insert(
            id,
            quill_pdf::PdfImage {
                width,
                height,
                rgba: rgba.to_vec(),
            },
        );
        Ok(())
    }

    /// Render the document as a PDF. Text uses the standard PDF fonts and is
    /// laid out with metric-compatible fonts, so line breaks can differ
    /// slightly from the on-screen layout.
    pub fn export_pdf(&mut self, title: &str) -> Result<Vec<u8>, JsError> {
        let f = self
            .measure
            .clone()
            .ok_or_else(|| JsError::new("set_measure must be called first"))?;
        let page = PageSetup::from_page(&self.inner.doc().page);
        let l = layout(self.inner.doc(), page, &PdfMeasure(&f));
        let pdf = quill_pdf::render(
            &l,
            page.width,
            page.height,
            &self.pdf_images,
            &quill_pdf::Options {
                title: title.to_owned(),
                compress: true,
            },
        );
        self.pdf_images.clear();
        Ok(pdf)
    }

    // ---- find and replace ------------------------------------------------

    /// Set the search; an empty query clears it.
    pub fn set_find(&mut self, query: &str, match_case: bool, whole_word: bool) {
        self.find = (!query.is_empty()).then(|| {
            (
                query.to_owned(),
                FindOptions {
                    match_case,
                    whole_word,
                },
            )
        });
    }

    fn search(&mut self, dir: FindDir) -> bool {
        self.goal_x = None;
        match self.find.clone() {
            Some((q, o)) => self.inner.find(&q, o, dir),
            None => false,
        }
    }

    /// Select the next (or previous) match, wrapping around.
    pub fn find_next(&mut self, backwards: bool) -> bool {
        self.search(if backwards {
            FindDir::Prev
        } else {
            FindDir::Next
        })
    }

    /// Select the first match at or after the caret (for search-as-you-type).
    pub fn find_current(&mut self) -> bool {
        self.search(FindDir::Current)
    }

    /// Replace the selected match and move to the next; returns whether one was replaced.
    pub fn replace_current(&mut self, with: &str) -> bool {
        self.goal_x = None;
        match self.find.clone() {
            Some((q, o)) => self.inner.replace_current(&q, o, with),
            None => false,
        }
    }

    /// Replace every match in one undo step; returns how many.
    pub fn replace_all(&mut self, with: &str) -> usize {
        self.goal_x = None;
        match self.find.clone() {
            Some((q, o)) => self.inner.replace_all(&q, o, with),
            None => 0,
        }
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
        // The page margins are where headers and footers are edited: a
        // double-click there enters them; a click in the text area leaves them.
        let setup = PageSetup::from_page(&self.inner.doc().page);
        let region = if y < setup.top {
            Story::Header
        } else if y > setup.height - setup.bottom {
            Story::Footer
        } else {
            Story::Body
        };
        if region != self.inner.story() && (clicks >= 2 || region == Story::Body) {
            self.inner.enter_story(region);
        }
        self.focus_page = page;
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
                story: match self.inner.story() {
                    Story::Body => "body",
                    Story::Header => "header",
                    Story::Footer => "footer",
                },
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
                    top: p.top,
                    bottom: p.bottom,
                }
            },
            find: match &self.find {
                Some((q, o)) => {
                    let matches = self.inner.doc().find_all(q, *o);
                    FindInfo {
                        rects: l.highlight_rects(&matches, &m),
                        count: matches.len(),
                        index: matches.iter().position(|&r| r == (s, e)),
                    }
                }
                None => FindInfo {
                    rects: vec![],
                    count: 0,
                    index: None,
                },
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
