//! Paginated line layout: greedy word wrap over styled runs.
//!
//! Text measurement is injected (`Measure`) so the same code runs against a
//! browser canvas, a native shaper, or a fixed-width stub in tests.
//!
//! All coordinates are page-local: `x`/`y` include the page margin, and each
//! line records the page it sits on.

use std::sync::Arc;

use quill_core::{
    Align, Document, Format, Image, Kind, Page, Paragraph, Pos, Story, OBJECT_CHAR, PAGE_CHAR,
};
use serde::Serialize;

/// Default body text size in CSS px (12 pt).
pub const BASE_SIZE: f32 = 16.0;
pub const LINE_FACTOR: f32 = 1.4;
pub const PARA_SPACING: f32 = 8.0;
pub const INDENT: f32 = 32.0;
const MARKER_GAP: f32 = 8.0;
const PARA_MARK_WIDTH: f32 = 6.0;
pub const CELL_PAD: f32 = 6.0;

#[derive(Clone, Debug)]
pub struct TextStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Size in CSS px.
    pub size: f32,
    pub font: Option<Arc<str>>,
    pub color: Option<u32>,
}

pub trait Measure {
    fn width(&self, text: &str, style: &TextStyle) -> f32;
}

impl<F: Fn(&str, &TextStyle) -> f32> Measure for F {
    fn width(&self, text: &str, style: &TextStyle) -> f32 {
        self(text, style)
    }
}

/// Page size and margins in CSS px.
#[derive(Clone, Copy, Debug)]
pub struct PageSetup {
    pub width: f32,
    pub height: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    /// Distance from the page edge to header/footer text.
    pub header: f32,
    pub footer: f32,
}

impl PageSetup {
    /// US Letter with one-inch margins.
    pub const LETTER: PageSetup = PageSetup::uniform(816.0, 1056.0, 96.0);

    pub const fn uniform(width: f32, height: f32, margin: f32) -> PageSetup {
        PageSetup {
            width,
            height,
            top: margin,
            right: margin,
            bottom: margin,
            left: margin,
            header: margin / 2.0,
            footer: margin / 2.0,
        }
    }

    pub fn from_page(p: &Page) -> PageSetup {
        PageSetup {
            width: Page::px(p.width),
            height: Page::px(p.height),
            top: Page::px(p.top),
            right: Page::px(p.right),
            bottom: Page::px(p.bottom),
            left: Page::px(p.left),
            header: Page::px(p.header),
            footer: Page::px(p.footer),
        }
    }
}

/// Convert half-points (the unit stored in documents) to CSS px.
pub fn hp_to_px(half_points: u16) -> f32 {
    half_points as f32 * 2.0 / 3.0
}

/// Default body text size of a document, in CSS px.
pub fn base_size(doc: &Document) -> f32 {
    doc.defaults.size.map_or(BASE_SIZE, hp_to_px)
}

/// Default text size of a paragraph kind, in CSS px, given the body size.
pub fn para_size(kind: Kind, base: f32) -> f32 {
    match kind {
        Kind::Heading(1) => hp_to_px(40),
        Kind::Heading(2) => hp_to_px(32),
        Kind::Heading(_) => hp_to_px(28),
        _ => base,
    }
}

/// Document-level inputs to paragraph layout.
struct Ctx<'a> {
    images: &'a [Image],
    base: f32,
    font: Option<Arc<str>>,
    story: Story,
    /// `(page number, page count)` for header/footer fields.
    fields: Option<(usize, usize)>,
}

fn style_for(kind: Kind, f: &Format, ctx: &Ctx) -> TextStyle {
    TextStyle {
        bold: f.bold || matches!(kind, Kind::Heading(_)),
        italic: f.italic,
        underline: f.underline,
        size: f.size.map_or_else(|| para_size(kind, ctx.base), hp_to_px),
        font: f.font.clone().or_else(|| ctx.font.clone()),
        color: f.color,
    }
}

fn twips_px(t: u16) -> f32 {
    t as f32 / 15.0
}

fn before_px(p: &Paragraph) -> f32 {
    p.space_before.map_or(0.0, twips_px)
}

fn after_px(p: &Paragraph) -> f32 {
    p.space_after.map_or(PARA_SPACING, twips_px)
}

/// An inline image as placed on a line (display size after fitting).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ImageBox {
    pub id: u32,
    /// Identifies the pixel data (for host-side caching).
    pub uid: u64,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    /// Absolute page x.
    pub x: f32,
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub size: f32,
    pub font: Option<Arc<str>>,
    pub color: Option<u32>,
    /// Measured width of `text`.
    pub w: f32,
    /// Char offset of `text` within its paragraph.
    pub start: usize,
    /// Set for an inline image; `text` is then a single object character.
    pub image: Option<ImageBox>,
    /// Occupies one model character whatever `text` shows (images, fields).
    pub atomic: bool,
}

impl Item {
    fn style(&self) -> TextStyle {
        TextStyle {
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            size: self.size,
            font: self.font.clone(),
            color: self.color,
        }
    }

    fn len(&self) -> usize {
        if self.atomic {
            1
        } else {
            self.text.chars().count()
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Marker {
    pub text: String,
    pub x: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Line {
    pub story: Story,
    pub para: usize,
    pub page: usize,
    /// Left edge of the text area (after list indent).
    pub x: f32,
    pub y: f32,
    pub height: f32,
    /// Distance from `y` to the text baseline.
    pub baseline: f32,
    /// Space below the baseline; `height == baseline + descent`.
    pub descent: f32,
    pub items: Vec<Item>,
    /// Paragraph char range covered by this line.
    pub start: usize,
    pub end: usize,
    pub marker: Option<Marker>,
    /// Horizontal extent of the container (page text area or table cell).
    pub left: f32,
    pub right: f32,
}

/// Which story the caret is in, and on which page it is shown (headers and
/// footers repeat on every page, so the page disambiguates).
#[derive(Clone, Copy, Debug, Default)]
pub struct Focus {
    pub story: Story,
    pub page: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Layout {
    pub lines: Vec<Line>,
    #[serde(skip)]
    pub focus: Focus,
    /// Table cell boxes, for drawing borders.
    pub cells: Vec<Rect>,
    pub pages: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Caret {
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub h: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Rect {
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Split into words, each keeping trailing whitespace; a '\n' ends a token.
fn tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_space = false;
    for (i, c) in text.char_indices() {
        if c == '\n' {
            let end = i + 1;
            out.push(&text[start..end]);
            start = end;
            in_space = false;
        } else if c.is_whitespace() {
            in_space = true;
        } else if in_space {
            out.push(&text[start..i]);
            start = i;
            in_space = false;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

struct Pager {
    lines: Vec<Line>,
    cells: Vec<Rect>,
    page: usize,
    /// Cursor within the page content area.
    y: f32,
    /// Top margin: where a page's content starts.
    top: f32,
    content_h: f32,
}

impl Pager {
    /// Start a new page if a block of height `h` doesn't fit in what's left.
    fn ensure_fits(&mut self, h: f32) {
        if self.y + h > self.content_h && self.y > 0.0 {
            self.page += 1;
            self.y = 0.0;
        }
    }

    fn push(&mut self, mut line: Line) {
        self.ensure_fits(line.height);
        line.page = self.page;
        line.y = self.top + self.y;
        self.y += line.height;
        self.lines.push(line);
    }
}

/// Add an item to a line, growing the line to fit it.
/// Split text into runs of ordinary text and single field placeholders.
fn segments(text: &str) -> Vec<(bool, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if quill_core::is_field_char(c) {
            if start < i {
                out.push((false, &text[start..i]));
            }
            out.push((true, &text[i..i + c.len_utf8()]));
            start = i + c.len_utf8();
        }
    }
    if start < text.len() {
        out.push((false, &text[start..]));
    }
    out
}

fn push_item(line: &mut Line, it: Item, ascent: f32, descent: f32) {
    if line.items.is_empty() {
        // The first item replaces the empty-line defaults.
        (line.baseline, line.descent) = (ascent, descent);
    } else {
        line.baseline = line.baseline.max(ascent);
        line.descent = line.descent.max(descent);
    }
    line.height = line.baseline + line.descent;
    line.items.push(it);
}

fn shift(line: &mut Line, dx: f32) {
    line.x += dx;
    for it in &mut line.items {
        it.x += dx;
    }
}

/// Apply paragraph alignment to finished lines. Trailing whitespace does not
/// count towards a line's width, and justify leaves the last line alone.
fn align_lines(
    lines: &mut [Line],
    align: Align,
    x0: f32,
    avail: f32,
    first_indent: f32,
    m: &dyn Measure,
) {
    if align == Align::Left {
        return;
    }
    let count = lines.len();
    for (li, line) in lines.iter_mut().enumerate() {
        // The first line may start indented (or hang to the left).
        let (edge, room) = if li == 0 {
            (x0 + first_indent, avail - first_indent)
        } else {
            (x0, avail)
        };
        let content_w = line.items.last().map_or(0.0, |it| {
            let trimmed = it.text.trim_end();
            let w = if it.atomic || trimmed.len() == it.text.len() {
                it.w
            } else {
                m.width(trimmed, &it.style())
            };
            it.x + w - edge
        });
        let slack = (room - content_w).max(0.0);
        match align {
            Align::Center => shift(line, slack / 2.0),
            Align::Right => shift(line, slack),
            Align::Justify => {
                let hard_break = line.items.last().is_some_and(|i| i.text.ends_with('\n'));
                let n = line.items.len();
                let gaps = line.items[..n.saturating_sub(1)]
                    .iter()
                    .filter(|i| !i.atomic && i.text.ends_with(char::is_whitespace))
                    .count();
                if li + 1 < count && !hard_break && gaps > 0 {
                    let extra = slack / gaps as f32;
                    let mut acc = 0.0;
                    for i in 0..n {
                        line.items[i].x += acc;
                        if i + 1 < n
                            && !line.items[i].atomic
                            && line.items[i].text.ends_with(char::is_whitespace)
                        {
                            acc += extra;
                        }
                    }
                }
            }
            Align::Left => {}
        }
    }
}

/// Lay out one paragraph inside a container spanning `[left, left + width]`.
/// Lines come back with `y` and `page` unset (0); the caller places them.
fn para_lines(
    pi: usize,
    para: &Paragraph,
    ctx: &Ctx,
    left: f32,
    width: f32,
    number: usize,
    m: &dyn Measure,
) -> Vec<Line> {
    let size = para_size(para.kind, ctx.base);
    let list = para.kind.is_list();
    let indent = if list {
        INDENT
    } else {
        para.indent_left.map_or(0.0, twips_px)
    };
    // The first line may be indented, or hang left (never outside the container).
    let first_px = if list {
        0.0
    } else {
        para.indent_first
            .map_or(0.0, |t| t as f32 / 15.0)
            .max(-indent)
    };
    let avail = width - indent;
    let x0 = left + indent;

    let marker = match para.kind {
        Kind::Bullet | Kind::Numbered => {
            let text = if para.kind == Kind::Bullet {
                "•".to_string()
            } else {
                format!("{number}.")
            };
            let st = style_for(para.kind, &Format::default(), ctx);
            Some(Marker {
                x: x0 - MARKER_GAP - m.width(&text, &st),
                text,
            })
        }
        _ => None,
    };
    // An empty line is as tall as the paragraph's default text.
    let new_line = |start: usize, marker: Option<Marker>| Line {
        story: ctx.story,
        para: pi,
        page: 0,
        x: x0,
        y: 0.0,
        height: size * LINE_FACTOR,
        baseline: size,
        descent: size * (LINE_FACTOR - 1.0),
        items: vec![],
        start,
        end: start,
        marker,
        left,
        right: left + width,
    };

    let mut lines = Vec::new();
    let mut cur = new_line(0, marker);
    cur.x += first_px;
    let (mut x, mut offset) = (first_px, 0usize);
    for run in &para.runs {
        let st = style_for(para.kind, &run.format, ctx);
        let descent = st.size * (LINE_FACTOR - 1.0);
        let item = |x: f32, text: &str, start: usize, w: f32| Item {
            x: x0 + x,
            text: text.to_string(),
            bold: st.bold,
            italic: st.italic,
            underline: st.underline,
            size: st.size,
            font: st.font.clone(),
            color: st.color,
            w,
            start,
            image: None,
            atomic: false,
        };
        if let Some(id) = run.format.image {
            // One inline image per object character.
            for _ in run.text.chars() {
                let (uid, mut w, mut h) = ctx
                    .images
                    .get(id as usize)
                    .map_or((0, st.size, st.size), |i| {
                        (i.uid, i.width as f32, i.height as f32)
                    });
                if w > avail {
                    h *= avail / w;
                    w = avail;
                }
                if x + w > avail && !cur.items.is_empty() {
                    cur.end = offset;
                    lines.push(std::mem::replace(&mut cur, new_line(offset, None)));
                    x = 0.0;
                }
                let mut it = item(x, &OBJECT_CHAR.to_string(), offset, w);
                it.image = Some(ImageBox { id, uid, w, h });
                it.atomic = true;
                // The image sits on the baseline; the line grows to hold it.
                push_item(&mut cur, it, h, descent);
                x += w;
                offset += 1;
            }
            continue;
        }
        for (is_field, piece) in segments(&run.text) {
            if is_field {
                let shown = match (piece.starts_with(PAGE_CHAR), ctx.fields) {
                    (_, None) => "#".to_string(),
                    (true, Some((page, _))) => page.to_string(),
                    (false, Some((_, pages))) => pages.to_string(),
                };
                let w = m.width(&shown, &st);
                if x + w > avail && !cur.items.is_empty() {
                    cur.end = offset;
                    lines.push(std::mem::replace(&mut cur, new_line(offset, None)));
                    x = 0.0;
                }
                let mut it = item(x, &shown, offset, w);
                it.atomic = true;
                push_item(&mut cur, it, st.size, descent);
                x += w;
                offset += 1;
                continue;
            }
            for tok in tokens(piece) {
                let n = tok.chars().count();
                let w = m.width(tok, &st);
                if x + w > avail && !cur.items.is_empty() {
                    cur.end = offset;
                    lines.push(std::mem::replace(&mut cur, new_line(offset, None)));
                    x = 0.0;
                }
                if w > avail {
                    // A single word wider than the line: break it by chars.
                    let chars: Vec<char> = tok.chars().collect();
                    let mut i = 0;
                    while i < chars.len() {
                        let mut j = i + 1;
                        while j < chars.len() {
                            let cand: String = chars[i..=j].iter().collect();
                            if m.width(&cand, &st) > avail {
                                break;
                            }
                            j += 1;
                        }
                        let chunk: String = chars[i..j].iter().collect();
                        let cw = m.width(&chunk, &st);
                        push_item(&mut cur, item(x, &chunk, offset + i, cw), st.size, descent);
                        x += cw;
                        i = j;
                        if i < chars.len() {
                            cur.end = offset + i;
                            lines.push(std::mem::replace(&mut cur, new_line(offset + i, None)));
                            x = 0.0;
                        }
                    }
                } else {
                    push_item(&mut cur, item(x, tok, offset, w), st.size, descent);
                    x += w;
                }
                offset += n;
                if tok.ends_with('\n') {
                    cur.end = offset;
                    lines.push(std::mem::replace(&mut cur, new_line(offset, None)));
                    x = 0.0;
                }
            }
        }
    }
    cur.end = offset;
    lines.push(cur);
    // Line spacing scales the natural line height. Quill's natural height
    // matches a 1.15 multiple, so a Word "single" line is slightly tighter.
    if let Some(l) = para.line_spacing {
        let mult = l as f32 / 240.0 / 1.15;
        for line in &mut lines {
            let h = (line.baseline + line.descent) * mult;
            line.descent = (h - line.baseline).max(line.baseline * 0.1);
            line.height = line.baseline + line.descent;
        }
    }
    align_lines(&mut lines, para.align, x0, avail, first_px, m);
    lines
}

pub fn layout(doc: &Document, setup: PageSetup, m: &dyn Measure) -> Layout {
    let content_w = setup.width - setup.left - setup.right;
    let paras = doc.story_paras(Story::Body);
    let ctx = Ctx {
        images: &doc.images,
        base: base_size(doc),
        font: doc.defaults.font.clone(),
        story: Story::Body,
        fields: None,
    };
    let mut pager = Pager {
        lines: vec![],
        cells: vec![],
        page: 0,
        y: 0.0,
        top: setup.top,
        content_h: setup.height - setup.top - setup.bottom,
    };
    let mut number = 0usize;
    let mut i = 0;
    while i < paras.len() {
        let para = &paras[i];
        let Some(first) = para.cell else {
            number = if para.kind == Kind::Numbered {
                number + 1
            } else {
                0
            };
            if pager.y > 0.0 {
                pager.y += match (para.space_before, para.kind) {
                    (None, Kind::Heading(_)) => para_size(para.kind, ctx.base) * 0.4,
                    _ => before_px(para),
                };
            }
            for line in para_lines(i, para, &ctx, setup.left, content_w, number, m) {
                pager.push(line);
            }
            pager.y += after_px(para);
            i += 1;
            continue;
        };

        // A table: consecutive paragraphs of one table id, row-major.
        number = 0;
        let mut j = i;
        let mut rows: Vec<Vec<Vec<usize>>> = Vec::new(); // row → col → paragraph indices
        let mut last: Option<(u16, u16)> = None;
        while j < paras.len() && paras[j].cell.is_some_and(|c| c.table == first.table) {
            let c = paras[j].cell.expect("checked");
            if last.map(|l| l.0) != Some(c.row) {
                rows.push(Vec::new());
            }
            if last != Some((c.row, c.col)) {
                rows.last_mut().expect("row exists").push(Vec::new());
            }
            rows.last_mut()
                .expect("row exists")
                .last_mut()
                .expect("cell exists")
                .push(j);
            last = Some((c.row, c.col));
            j += 1;
        }
        let ncols = rows.iter().map(Vec::len).max().unwrap_or(1);
        let col_w = content_w / ncols as f32;
        for row in &rows {
            let mut cell_lines: Vec<Vec<Line>> = Vec::new();
            let mut row_h = 0.0f32;
            for (ci, cell) in row.iter().enumerate() {
                let left = setup.left + ci as f32 * col_w + CELL_PAD;
                let (mut y, mut n, mut lines) = (0.0f32, 0usize, Vec::new());
                let mut trailing = 0.0;
                for &pi in cell {
                    let p = &paras[pi];
                    n = if p.kind == Kind::Numbered { n + 1 } else { 0 };
                    y += before_px(p);
                    for mut line in para_lines(pi, p, &ctx, left, col_w - 2.0 * CELL_PAD, n, m) {
                        line.y = y;
                        y += line.height;
                        lines.push(line);
                    }
                    trailing = after_px(p);
                    y += trailing;
                }
                row_h = row_h.max(y - trailing);
                cell_lines.push(lines);
            }
            row_h += 2.0 * CELL_PAD;
            pager.ensure_fits(row_h);
            let top = pager.top + pager.y;
            for (ci, lines) in cell_lines.into_iter().enumerate() {
                pager.cells.push(Rect {
                    page: pager.page,
                    x: setup.left + ci as f32 * col_w,
                    y: top,
                    w: col_w,
                    h: row_h,
                });
                for mut line in lines {
                    line.page = pager.page;
                    line.y += top + CELL_PAD;
                    pager.lines.push(line);
                }
            }
            pager.y += row_h;
        }
        pager.y += PARA_SPACING;
        i = j;
    }
    let pages = pager.page + 1;
    let mut lines = pager.lines;

    // Headers and footers repeat on every page, each with its own page number.
    let empty = [Paragraph::default()];
    for story in [Story::Header, Story::Footer] {
        let src = doc.story_paras(story);
        let src = if src.is_empty() { &empty[..] } else { src };
        for page in 0..pages {
            let hf_ctx = Ctx {
                images: &doc.images,
                base: ctx.base,
                font: ctx.font.clone(),
                story,
                fields: Some((page + 1, pages)),
            };
            let (mut y, mut number, mut local) = (0.0f32, 0usize, Vec::new());
            for (pi, p) in src.iter().enumerate() {
                number = if p.kind == Kind::Numbered {
                    number + 1
                } else {
                    0
                };
                y += before_px(p);
                for mut line in para_lines(pi, p, &hf_ctx, setup.left, content_w, number, m) {
                    line.y = y;
                    y += line.height;
                    local.push(line);
                }
                // No default paragraph gap: footer text should sit on its margin.
                y += p.space_after.map_or(0.0, twips_px);
            }
            let top = match story {
                Story::Footer => setup.height - setup.footer - y,
                _ => setup.header,
            };
            for mut line in local {
                line.page = page;
                line.y += top;
                lines.push(line);
            }
        }
    }
    Layout {
        lines,
        cells: pager.cells,
        pages,
        focus: Focus::default(),
    }
}

/// Width of the first `chars` characters of an item.
fn prefix_width(it: &Item, chars: usize, m: &dyn Measure) -> f32 {
    if it.atomic {
        if chars == 0 {
            0.0
        } else {
            it.w
        }
    } else {
        let prefix: String = it.text.chars().take(chars).collect();
        m.width(&prefix, &it.style())
    }
}

impl Layout {
    /// Whether lines `a` and `b` belong to the same paragraph (header and
    /// footer paragraphs repeat on each page, so the page counts too).
    fn same_para(a: &Line, b: &Line) -> bool {
        a.story == b.story && a.para == b.para && (a.story == Story::Body || a.page == b.page)
    }

    fn is_last_line(&self, li: usize) -> bool {
        self.lines
            .get(li + 1)
            .is_none_or(|n| !Self::same_para(n, &self.lines[li]))
    }

    /// Whether a line is in the story (and, for header/footer, the page) the
    /// caret is in.
    fn in_focus(&self, l: &Line) -> bool {
        l.story == self.focus.story && (l.story == Story::Body || l.page == self.focus.page)
    }

    /// Largest offset the caret may take on a line: not past a soft-wrap space.
    fn max_offset(&self, li: usize) -> usize {
        let line = &self.lines[li];
        if self.is_last_line(li) {
            return line.end;
        }
        match line.items.last() {
            Some(it) if it.text.ends_with(char::is_whitespace) && line.end > line.start => {
                line.end - 1
            }
            _ => line.end,
        }
    }

    /// Index of the line holding `pos`; a caret at a wrap point belongs to the
    /// start of the next line.
    pub fn line_index(&self, pos: Pos) -> usize {
        let holds = |l: &Line| l.para == pos.para && l.start <= pos.offset;
        self.lines
            .iter()
            .rposition(|l| self.in_focus(l) && holds(l))
            // The focused page may not exist (e.g. after the document shrank).
            .or_else(|| {
                self.lines
                    .iter()
                    .rposition(|l| l.story == self.focus.story && holds(l))
            })
            .unwrap_or(self.lines.len() - 1)
    }

    fn x_at(&self, li: usize, offset: usize, m: &dyn Measure) -> f32 {
        let line = &self.lines[li];
        let mut x = line.x;
        for it in &line.items {
            let n = it.len();
            if offset < it.start {
                break;
            }
            if offset <= it.start + n {
                x = it.x + prefix_width(it, offset - it.start, m);
                if offset < it.start + n {
                    break;
                }
            }
        }
        x
    }

    pub fn caret(&self, pos: Pos, m: &dyn Measure) -> Caret {
        let li = self.line_index(pos);
        let l = &self.lines[li];
        Caret {
            page: l.page,
            x: self.x_at(li, pos.offset, m),
            y: l.y,
            h: l.height,
        }
    }

    fn hit_line(&self, li: usize, x: f32, m: &dyn Measure) -> Pos {
        let line = &self.lines[li];
        let mut off = line.start;
        for it in &line.items {
            if x < it.x {
                break;
            }
            let mut prev = 0.0;
            for k in 1..=it.len() {
                let w = prefix_width(it, k, m);
                if x >= it.x + (prev + w) / 2.0 {
                    off = it.start + k;
                }
                prev = w;
            }
        }
        Pos {
            para: line.para,
            offset: off.min(self.max_offset(li)),
        }
    }

    /// Map a page-local point to a document position: the line nearest the
    /// point vertically, then horizontally (so table cells resolve by column).
    pub fn hit_test(&self, page: usize, x: f32, y: f32, m: &dyn Measure) -> Pos {
        let mut best: Option<(usize, f32, f32)> = None;
        for (i, l) in self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.page == page && l.story == self.focus.story)
        {
            let dy = if y < l.y {
                l.y - y
            } else {
                (y - (l.y + l.height)).max(0.0)
            };
            let dx = if x < l.left {
                l.left - x
            } else {
                (x - l.right).max(0.0)
            };
            let better = match best {
                None => true,
                Some((_, bdy, bdx)) => dy < bdy - 0.01 || ((dy - bdy).abs() <= 0.01 && dx < bdx),
            };
            if better {
                best = Some((i, dy, dx));
            }
        }
        let li = best.map_or(self.lines.len() - 1, |b| b.0);
        self.hit_line(li, x, m)
    }

    /// Caret one visual line up (`dir < 0`) or down, keeping horizontal
    /// `goal_x`. Chooses geometrically, so it moves within a table column.
    pub fn vertical(&self, pos: Pos, dir: i32, goal_x: f32, m: &dyn Measure) -> Pos {
        let li = self.line_index(pos);
        let cur = &self.lines[li];
        let top = |l: &Line| l.page as f32 * 1.0e6 + l.y;
        let mut best: Option<(usize, bool, f32, f32)> = None; // (line, in column, gap, dx)
        for (i, l) in self.lines.iter().enumerate() {
            if !self.in_focus(l) {
                continue;
            }
            let gap = if dir > 0 {
                top(l) - (top(cur) + cur.height)
            } else {
                top(cur) - (top(l) + l.height)
            };
            if gap < -0.5 {
                continue;
            }
            let dx = if goal_x < l.left {
                l.left - goal_x
            } else {
                (goal_x - l.right).max(0.0)
            };
            let in_col = dx == 0.0;
            let better = match best {
                None => true,
                Some((_, bc, bg, bdx)) => {
                    if in_col != bc {
                        in_col
                    } else if (gap - bg).abs() > 0.01 {
                        gap < bg
                    } else {
                        dx < bdx
                    }
                }
            };
            if better {
                best = Some((i, in_col, gap, dx));
            }
        }
        match best {
            Some((i, ..)) => self.hit_line(i, goal_x, m),
            // First/last line: jump to its start/end.
            None => self.hit_line(li, if dir < 0 { f32::MIN } else { f32::MAX }, m),
        }
    }

    /// Caret range `(start, end)` of the visual line containing `pos`.
    pub fn line_range(&self, pos: Pos) -> (usize, usize) {
        let li = self.line_index(pos);
        (self.lines[li].start, self.max_offset(li))
    }

    pub fn selection_rects(&self, start: Pos, end: Pos, m: &dyn Measure) -> Vec<Rect> {
        let mut out = Vec::new();
        if start >= end {
            return out;
        }
        for (li, l) in self.lines.iter().enumerate() {
            if !self.in_focus(l) || l.para < start.para || l.para > end.para {
                continue;
            }
            let lo = if l.para == start.para {
                start.offset
            } else {
                0
            }
            .max(l.start);
            let hi = if l.para == end.para {
                end.offset
            } else {
                usize::MAX
            }
            .min(self.max_offset(li));
            let breaks = l.para < end.para && self.is_last_line(li);
            if lo > hi || (lo == hi && !breaks) {
                continue;
            }
            let x1 = self.x_at(li, lo, m);
            let x2 = self.x_at(li, hi, m) + if breaks { PARA_MARK_WIDTH } else { 0.0 };
            out.push(Rect {
                page: l.page,
                x: x1,
                y: l.y,
                w: x2 - x1,
                h: l.height,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layout restricted to the body, so counts and indexes ignore the
    /// header and footer lines present on every page.
    fn lay(doc: &Document, setup: PageSetup, m: &dyn Measure) -> Layout {
        let mut l = layout(doc, setup, m);
        l.lines.retain(|x| x.story == Story::Body);
        l
    }
    use quill_core::{Align, Paragraph};

    /// Height of a default text line.
    const LH: f32 = BASE_SIZE * LINE_FACTOR;

    // 10px per char; ignores style.
    fn m(t: &str, _: &TextStyle) -> f32 {
        t.chars().count() as f32 * 10.0
    }

    // 100px content width, 40px margins, 4 lines (of LHpx) per page.
    const SETUP: PageSetup = PageSetup::uniform(180.0, 80.0 + 4.0 * LH + 1.0, 40.0);

    fn doc(texts: &[&str]) -> Document {
        Document::from_paragraphs(texts.iter().map(|t| Paragraph::plain(t)).collect())
    }

    #[test]
    fn wraps_at_width() {
        let d = doc(&["aaaa bbbb cccc"]);
        let l = lay(&d, SETUP, &m);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[1].items[0].text, "cccc");
        assert_eq!(l.lines[1].start, 10);
    }

    #[test]
    fn empty_paragraph_has_a_line() {
        assert_eq!(lay(&Document::default(), SETUP, &m).lines.len(), 1);
    }

    #[test]
    fn long_word_is_broken_by_chars() {
        let d = doc(&["abcdefghijklmnopqrstuvwxyz"]);
        let l = lay(&d, SETUP, &m);
        let texts: Vec<_> = l.lines.iter().map(|l| l.items[0].text.as_str()).collect();
        assert_eq!(texts, ["abcdefghij", "klmnopqrst", "uvwxyz"]);
        assert_eq!(l.lines[2].start, 20);
    }

    #[test]
    fn forced_newline_breaks_line() {
        let mut d = doc(&["ab"]);
        d.paragraphs[0].runs[0].text = "ab\ncd".into();
        let l = lay(&d, SETUP, &m);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[1].start, 3);
    }

    #[test]
    fn paginates() {
        let d = doc(&["a", "b", "c", "d", "e"]);
        let l = lay(&d, SETUP, &m);
        assert!(l.pages >= 2, "pages = {}", l.pages);
        assert_eq!(l.lines[0].page, 0);
        assert_eq!(l.lines.last().unwrap().page, l.pages - 1);
        let first_on_p1 = l.lines.iter().find(|l| l.page == 1).unwrap();
        assert_eq!(
            first_on_p1.y, SETUP.left,
            "new page starts at the top margin"
        );
        assert_eq!(first_on_p1.para, 3);
    }

    #[test]
    fn list_markers_and_indent() {
        let d = Document::from_paragraphs(vec![
            Paragraph::plain("a").with_kind(Kind::Numbered),
            Paragraph::plain("b").with_kind(Kind::Numbered),
            Paragraph::plain("c"),
            Paragraph::plain("d").with_kind(Kind::Numbered),
            Paragraph::plain("e").with_kind(Kind::Bullet),
        ]);
        let l = lay(&d, SETUP, &m);
        let marks: Vec<_> = l
            .lines
            .iter()
            .map(|l| l.marker.as_ref().map(|m| m.text.clone()))
            .collect();
        assert_eq!(marks[0].as_deref(), Some("1."));
        assert_eq!(marks[1].as_deref(), Some("2."));
        assert_eq!(marks[2], None);
        assert_eq!(
            marks[3].as_deref(),
            Some("1."),
            "numbering restarts after a normal paragraph"
        );
        assert_eq!(marks[4].as_deref(), Some("•"));
        assert_eq!(l.lines[0].items[0].x, SETUP.left + INDENT);
    }

    #[test]
    fn heading_is_larger_and_bold() {
        let d = Document::from_paragraphs(vec![Paragraph::plain("T").with_kind(Kind::Heading(1))]);
        let l = lay(&d, SETUP, &m);
        assert_eq!(
            l.lines[0].items[0].size,
            para_size(Kind::Heading(1), BASE_SIZE)
        );
        assert!(l.lines[0].items[0].bold);
        assert!(l.lines[0].height > BASE_SIZE * LINE_FACTOR);
    }

    #[test]
    fn caret_at_wrap_point_is_on_next_line() {
        let d = doc(&["aaaa bbbb cccc"]);
        let l = lay(&d, SETUP, &m);
        let c = l.caret(
            Pos {
                para: 0,
                offset: 10,
            },
            &m,
        );
        assert_eq!((c.x, c.y), (SETUP.left, SETUP.left + LH));
    }

    #[test]
    fn hit_test_round_trips_caret_across_pages() {
        let d = doc(&["aaaa bbbb cccc", "xyz", "q", "r", "s"]);
        let l = lay(&d, SETUP, &m);
        assert!(l.pages > 1);
        for pos in [
            Pos { para: 0, offset: 3 },
            Pos {
                para: 0,
                offset: 12,
            },
            Pos { para: 1, offset: 2 },
            Pos { para: 4, offset: 1 },
        ] {
            let c = l.caret(pos, &m);
            assert_eq!(l.hit_test(c.page, c.x, c.y + 1.0, &m), pos);
        }
    }

    #[test]
    fn click_past_wrapped_line_end_stays_on_that_line() {
        let d = doc(&["aaaa bbbb cccc"]);
        let l = lay(&d, SETUP, &m);
        let p = l.hit_test(0, 500.0, SETUP.left + 1.0, &m);
        assert_eq!(p, Pos { para: 0, offset: 9 }, "before the soft-wrap space");
    }

    #[test]
    fn vertical_keeps_goal_x() {
        let d = doc(&["aaaa bbbb cccc", "xy"]);
        let l = lay(&d, SETUP, &m);
        let from = Pos { para: 0, offset: 3 };
        let gx = l.caret(from, &m).x;
        let down = l.vertical(from, 1, gx, &m);
        assert_eq!(
            down,
            Pos {
                para: 0,
                offset: 13
            }
            .min(down)
        ); // second line, clamped to its length
        assert_eq!(l.line_index(down), 1);
        let down2 = l.vertical(down, 1, gx, &m);
        assert_eq!(down2, Pos { para: 1, offset: 2 }); // "xy" is shorter than goal_x
        assert_eq!(l.vertical(down2, -1, gx, &m).para, 0);
        // At the very top/bottom the caret jumps to the line edge.
        assert_eq!(l.vertical(from, -1, gx, &m), Pos { para: 0, offset: 0 });
        assert_eq!(l.vertical(down2, 1, gx, &m), Pos { para: 1, offset: 2 });
    }

    #[test]
    fn selection_rects_cover_lines_and_paragraph_breaks() {
        let d = doc(&["aaaa bbbb cccc", "xy"]);
        let l = lay(&d, SETUP, &m);
        let r = l.selection_rects(Pos { para: 0, offset: 2 }, Pos { para: 1, offset: 1 }, &m);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].x, SETUP.left + 20.0);
        assert!(
            r[1].w > 0.0,
            "second line is selected through its paragraph mark"
        );
        assert_eq!(r[2].w, 10.0);
        assert!(l
            .selection_rects(Pos::default(), Pos::default(), &m)
            .is_empty());
    }

    #[test]
    fn line_range_excludes_soft_wrap_space() {
        let d = doc(&["aaaa bbbb cccc"]);
        let l = lay(&d, SETUP, &m);
        assert_eq!(l.line_range(Pos { para: 0, offset: 2 }), (0, 9));
        assert_eq!(
            l.line_range(Pos {
                para: 0,
                offset: 12
            }),
            (10, 14)
        );
    }

    fn table_doc(rows: &[&[&str]]) -> Document {
        let mut ps = vec![Paragraph::plain("before")];
        for (r, row) in rows.iter().enumerate() {
            for (c, t) in row.iter().enumerate() {
                let mut p = Paragraph::plain(t);
                p.cell = Some(quill_core::Cell {
                    table: 0,
                    row: r as u16,
                    col: c as u16,
                });
                ps.push(p);
            }
        }
        ps.push(Paragraph::plain("after"));
        Document::from_paragraphs(ps)
    }

    // Roomy page: 300 wide, 40 margin → 220 content width.
    const WIDE: PageSetup = PageSetup::uniform(300.0, 600.0, 40.0);

    #[test]
    fn table_cells_share_row_height_and_columns() {
        let d = table_doc(&[&["a", "b c d e f g h i j k l m n o p q r s t"], &["x", "y"]]);
        let l = lay(&d, WIDE, &m);
        assert_eq!(l.cells.len(), 4);
        let col_w = 220.0 / 2.0;
        assert_eq!(l.cells[0].w, col_w);
        assert_eq!(l.cells[1].x, 40.0 + col_w);
        assert_eq!(
            l.cells[0].h, l.cells[1].h,
            "cells in a row are equally tall"
        );
        assert!(
            l.cells[0].h > LH + 2.0 * CELL_PAD,
            "tall cell forces taller row"
        );
        assert!(
            l.cells[2].y >= l.cells[0].y + l.cells[0].h,
            "next row sits below"
        );
        let a = l
            .lines
            .iter()
            .find(|l| l.items.first().is_some_and(|i| i.text == "a"))
            .unwrap();
        assert_eq!(a.x, 40.0 + CELL_PAD);
        assert_eq!(a.right, 40.0 + col_w - CELL_PAD);
        let after = l
            .lines
            .iter()
            .find(|l| l.para == d.paragraphs.len() - 1)
            .unwrap();
        assert!(
            after.y > l.cells[3].y + l.cells[3].h,
            "text after the table follows it"
        );
    }

    #[test]
    fn table_rows_move_to_next_page_whole() {
        let rows: Vec<&[&str]> = vec![&["a", "b"]; 12];
        let d = table_doc(&rows);
        let tight = PageSetup::uniform(300.0, 40.0 * 2.0 + 150.0, 40.0);
        let l = lay(&d, tight, &m);
        assert!(l.pages > 1);
        for c in &l.cells {
            assert!(
                c.y + c.h <= tight.height - tight.bottom + 0.01,
                "no row straddles the bottom margin: {c:?}"
            );
        }
        let page1_top = l.cells.iter().find(|c| c.page == 1).unwrap();
        assert_eq!(page1_top.y, tight.top);
    }

    #[test]
    fn hit_test_and_vertical_follow_columns() {
        let d = table_doc(&[&["aaa", "bbb"], &["ccc", "ddd"]]);
        let l = lay(&d, WIDE, &m);
        // paragraphs: 0 before, 1 aaa, 2 bbb, 3 ccc, 4 ddd, 5 after
        let right_col_x = 40.0 + 110.0 + CELL_PAD + 5.0;
        let top = l.lines.iter().find(|l| l.para == 2).unwrap();
        assert_eq!(l.hit_test(0, right_col_x, top.y + 1.0, &m).para, 2);
        assert_eq!(l.hit_test(0, 50.0, top.y + 1.0, &m).para, 1);
        // Down from "bbb" stays in the right column: → "ddd", not "ccc".
        let gx = l.caret(Pos { para: 2, offset: 1 }, &m).x;
        let down = l.vertical(Pos { para: 2, offset: 1 }, 1, gx, &m);
        assert_eq!((down.para, down.offset), (4, 1));
        // Down again leaves the table for the paragraph after it.
        assert_eq!(l.vertical(down, 1, gx, &m).para, 5);
        // Up from "ddd" goes back to "bbb".
        assert_eq!(l.vertical(down, -1, gx, &m).para, 2);
    }

    #[test]
    fn numbered_lists_restart_in_each_cell() {
        let mut d = table_doc(&[&["a", "b"]]);
        for i in [1, 2] {
            d.paragraphs[i].kind = Kind::Numbered;
        }
        let l = lay(&d, WIDE, &m);
        let marks: Vec<_> = l
            .lines
            .iter()
            .filter_map(|l| l.marker.as_ref().map(|m| m.text.as_str()))
            .collect();
        assert_eq!(marks, ["1.", "1."]);
    }

    fn image_doc(w: u32, h: u32) -> Document {
        let mut d = Document::from_paragraphs(vec![Paragraph::plain("ab cd")]);
        d.insert_image(
            Pos { para: 0, offset: 2 },
            Image::new("image/png", vec![1], w, h),
        );
        d
    }

    #[test]
    fn inline_image_grows_line_and_sits_on_baseline() {
        let d = image_doc(50, 100);
        let l = lay(&d, SETUP, &m);
        let line = &l.lines[0];
        let img = line.items.iter().find(|i| i.image.is_some()).unwrap();
        assert_eq!(img.image.unwrap().h, 100.0);
        assert_eq!(line.baseline, 100.0, "baseline lowered to the image bottom");
        assert!(
            line.height > 100.0 && line.height < 100.0 + BASE_SIZE,
            "line = image + descent"
        );
        // Text after the image continues on the same line (offset 3 = after the image).
        let after = line.items.iter().find(|i| i.start == 3).unwrap();
        assert_eq!(after.x, img.x + 50.0);
    }

    #[test]
    fn oversized_image_is_scaled_to_the_line_width() {
        let d = image_doc(400, 200);
        let l = lay(&d, SETUP, &m); // 100px wide text area
        let img = l
            .lines
            .iter()
            .flat_map(|l| &l.items)
            .find_map(|i| i.image)
            .unwrap();
        assert_eq!((img.w, img.h), (100.0, 50.0));
    }

    #[test]
    fn image_wraps_like_a_word() {
        let d = image_doc(60, 20); // "ab" (20px) + image (60) fits; then " cd" would not on same line
        let l = lay(&d, SETUP, &m);
        assert!(l.lines.len() >= 2);
        assert!(l
            .lines
            .iter()
            .flat_map(|l| &l.items)
            .any(|i| i.image.is_some()));
    }

    #[test]
    fn caret_and_hit_test_around_an_image() {
        let d = image_doc(30, 20);
        let l = lay(&d, SETUP, &m);
        let before = l.caret(Pos { para: 0, offset: 2 }, &m);
        let after = l.caret(Pos { para: 0, offset: 3 }, &m);
        assert_eq!(after.x - before.x, 30.0);
        let y = before.y + 5.0;
        assert_eq!(
            l.hit_test(0, before.x + 5.0, y, &m).offset,
            2,
            "left half → before"
        );
        assert_eq!(
            l.hit_test(0, before.x + 25.0, y, &m).offset,
            3,
            "right half → after"
        );
        let r = l.selection_rects(Pos { para: 0, offset: 2 }, Pos { para: 0, offset: 3 }, &m);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].w, 30.0);
    }

    fn sized(runs: &[(&str, Option<u16>)]) -> Document {
        let mut p = Paragraph::default();
        for (t, size) in runs {
            p.runs.push(quill_core::Run::new(
                *t,
                Format {
                    size: *size,
                    ..Format::default()
                },
            ));
        }
        Document::from_paragraphs(vec![p])
    }

    #[test]
    fn mixed_sizes_share_a_baseline_and_grow_the_line() {
        // 48 half-points = 24pt = 32px.
        let d = sized(&[("ab ", None), ("CD", Some(48)), (" ef", None)]);
        let l = lay(&d, SETUP, &m);
        let line = &l.lines[0];
        let sizes: Vec<_> = line.items.iter().map(|i| i.size).collect();
        assert_eq!(
            sizes,
            [BASE_SIZE, 32.0, BASE_SIZE, BASE_SIZE],
            "ab | CD | space | ef"
        );
        assert_eq!(line.baseline, 32.0, "tallest text sets the baseline");
        assert!((line.height - 32.0 * LINE_FACTOR).abs() < 1e-3);
        // A line of only small text is back to the default height.
        let small = lay(&sized(&[("ab", None)]), SETUP, &m);
        assert!((small.lines[0].height - LH).abs() < 1e-3);
        // Lone small text in a paragraph is shorter than the default line.
        let tiny = lay(&sized(&[("ab", Some(12))]), SETUP, &m); // 6pt = 8px
        assert!(tiny.lines[0].height < LH);
    }

    #[test]
    fn font_and_color_reach_items() {
        let mut p = Paragraph::plain("hi");
        p.runs[0].format.font = Some("Courier New".into());
        p.runs[0].format.color = Some(0xFF0000);
        let l = lay(&Document::from_paragraphs(vec![p]), SETUP, &m);
        let it = &l.lines[0].items[0];
        assert_eq!(it.font.as_deref(), Some("Courier New"));
        assert_eq!(it.color, Some(0xFF0000));
    }

    fn aligned(text: &str, align: Align) -> Layout {
        let mut p = Paragraph::plain(text);
        p.align = align;
        lay(&Document::from_paragraphs(vec![p]), SETUP, &m)
    }

    #[test]
    fn center_and_right_ignore_trailing_space() {
        // width 100; "ab cd" fits on a line: "ab " + "cd" = 50 wide.
        let c = aligned("ab cd", Align::Center);
        assert_eq!(c.lines[0].items[0].x, SETUP.left + 25.0);
        let r = aligned("ab cd", Align::Right);
        assert_eq!(
            r.lines[0].items[1].x + 20.0,
            SETUP.left + 100.0,
            "text ends at the right edge"
        );
        // Trailing space on the last item is not counted.
        let t = aligned("ab cd ", Align::Right);
        assert_eq!(t.lines[0].items[1].x + 20.0, SETUP.left + 100.0);
    }

    #[test]
    fn caret_follows_alignment_even_on_empty_lines() {
        let c = aligned("", Align::Center);
        let caret = c.caret(Pos::default(), &m);
        assert_eq!(caret.x, SETUP.left + 50.0);
        let r = aligned("ab", Align::Right);
        assert_eq!(
            r.caret(Pos { para: 0, offset: 2 }, &m).x,
            SETUP.left + 100.0
        );
        assert_eq!(
            r.hit_test(0, SETUP.left + 95.0, SETUP.left + 2.0, &m)
                .offset,
            2
        );
    }

    #[test]
    fn justify_stretches_wrapped_lines_but_not_the_last_or_forced_breaks() {
        // "aaaa bbbb cccc": line 1 is "aaaa bbbb" (trimmed width 90, one gap).
        let j = aligned("aaaa bbbb cccc", Align::Justify);
        assert_eq!(j.lines.len(), 2);
        let first = &j.lines[0];
        assert_eq!(first.items[0].x, SETUP.left);
        assert_eq!(
            first.items[1].x + 40.0,
            SETUP.left + 100.0,
            "last word reaches the right edge"
        );
        assert_eq!(
            j.lines[1].items[0].x, SETUP.left,
            "last line stays left-aligned"
        );

        let mut p = Paragraph::plain("a b\nc d");
        p.align = Align::Justify;
        let l = lay(&Document::from_paragraphs(vec![p]), SETUP, &m);
        assert_eq!(
            l.lines[0].items[1].x,
            SETUP.left + 20.0,
            "line ended by a break is not stretched"
        );
    }

    #[test]
    fn alignment_applies_inside_list_indent_and_table_cells() {
        let mut p = Paragraph::plain("ab").with_kind(Kind::Bullet);
        p.align = Align::Right;
        let l = lay(&Document::from_paragraphs(vec![p]), SETUP, &m);
        assert_eq!(l.lines[0].items[0].x + 20.0, SETUP.left + 100.0);
        assert!(
            l.lines[0].marker.as_ref().unwrap().x < SETUP.left + INDENT,
            "marker stays at the indent"
        );

        let mut d = table_doc(&[&["ab", "cd"]]);
        d.paragraphs[1].align = Align::Right;
        let l = lay(&d, WIDE, &m);
        let ab = l
            .lines
            .iter()
            .find(|l| l.items.first().is_some_and(|i| i.text == "ab"))
            .unwrap();
        assert_eq!(ab.items[0].x + 20.0, ab.right);
    }

    #[test]
    fn asymmetric_margins_position_and_wrap_text() {
        // 300 wide; left 60, right 20 → 220 wide text area starting at x=60.
        let setup = PageSetup {
            width: 300.0,
            height: 400.0,
            top: 30.0,
            right: 20.0,
            bottom: 50.0,
            left: 60.0,
            header: 10.0,
            footer: 10.0,
        };
        let d = doc(&["aaaaaaaaa bbbbbbbbbb cccccccccc"]); // 10px/char: 3 words of ~100px
        let l = lay(&d, setup, &m);
        assert_eq!(l.lines[0].items[0].x, 60.0);
        assert_eq!(l.lines[0].y, 30.0);
        assert_eq!(l.lines[0].right, 280.0);
        assert_eq!(l.lines.len(), 2, "two words fit in 220px, the third wraps");
        let tall = doc(&vec!["x"; 40]);
        let l = lay(&tall, setup, &m);
        let last_on_p0 = l.lines.iter().rev().find(|l| l.page == 0).unwrap();
        assert!(
            last_on_p0.y + last_on_p0.height <= 400.0 - 50.0 + 0.01,
            "bottom margin respected"
        );
    }

    #[test]
    fn document_defaults_set_size_and_font() {
        let mut d = doc(&["ab"]);
        d.defaults.size = Some(30); // 15pt = 20px
        d.defaults.font = Some("Calibri".into());
        let l = lay(&d, SETUP, &m);
        let it = &l.lines[0].items[0];
        assert_eq!((it.size, it.font.as_deref()), (20.0, Some("Calibri")));
        assert!((l.lines[0].height - 20.0 * LINE_FACTOR).abs() < 1e-3);
        // An explicit run font wins; headings keep their own size.
        d.paragraphs[0].runs[0].format.font = Some("Courier New".into());
        assert_eq!(
            lay(&d, SETUP, &m).lines[0].items[0].font.as_deref(),
            Some("Courier New")
        );
        d.paragraphs[0].kind = Kind::Heading(1);
        assert_eq!(lay(&d, SETUP, &m).lines[0].items[0].size, hp_to_px(40));
    }

    #[test]
    fn paragraph_spacing_before_and_after() {
        let mut d = doc(&["a", "b", "c"]);
        d.paragraphs[0].space_after = Some(300); // 20px
        d.paragraphs[1].space_before = Some(150); // 10px
        d.paragraphs[1].space_after = Some(0);
        let l = lay(&d, WIDE, &m);
        let ys: Vec<f32> = l.lines.iter().map(|l| l.y).collect();
        assert!(
            (ys[1] - ys[0] - (LH + 20.0 + 10.0)).abs() < 1e-3,
            "after + before add up: {ys:?}"
        );
        assert!((ys[2] - ys[1] - LH).abs() < 1e-3, "explicit zero spacing");
        // Space before is dropped at the very top of the page.
        let mut top = doc(&["a"]);
        top.paragraphs[0].space_before = Some(600);
        assert_eq!(lay(&top, WIDE, &m).lines[0].y, WIDE.top);
    }

    #[test]
    fn line_spacing_scales_line_height() {
        let one = |l: Option<u16>| {
            let mut d = doc(&["aaaa bbbb cccc"]);
            d.paragraphs[0].line_spacing = l;
            lay(&d, SETUP, &m).lines[0].height
        };
        let (def, single, double) = (one(None), one(Some(240)), one(Some(480)));
        assert!(single < def, "Word single is tighter than Quill's default");
        assert!((double / single - 2.0).abs() < 0.01);
        assert!(
            (one(Some(276)) - def).abs() < 1e-3,
            "1.15 matches the default look"
        );
        // Text keeps its baseline; the extra space goes below it.
        let mut d = doc(&["ab"]);
        d.paragraphs[0].line_spacing = Some(480);
        let line = &lay(&d, SETUP, &m).lines[0].clone();
        assert_eq!(line.baseline, BASE_SIZE);
        assert!((line.height - line.baseline - line.descent).abs() < 1e-3);
    }

    #[test]
    fn indents_shift_and_narrow_the_text_area() {
        let mut d = doc(&["aaaa bbbb cccc"]);
        d.paragraphs[0].indent_left = Some(300); // 20px
        let l = lay(&d, SETUP, &m); // 100px - 20px = 80px: one 4-char word + space per line? "aaaa bbbb" = 90 > 80
        assert_eq!(l.lines[0].items[0].x, SETUP.left + 20.0);
        assert_eq!(l.lines.len(), 3);

        // First-line indent shifts only the first line.
        d.paragraphs[0].indent_left = None;
        d.paragraphs[0].indent_first = Some(150); // 10px
        let l = lay(&d, SETUP, &m);
        assert_eq!(l.lines[0].items[0].x, SETUP.left + 10.0);
        assert_eq!(l.lines[1].items[0].x, SETUP.left);

        // Hanging indent: first line sits left of the rest.
        d.paragraphs[0].indent_left = Some(450); // 30px
        d.paragraphs[0].indent_first = Some(-300); // hang 20px
        let l = lay(&d, SETUP, &m);
        assert_eq!(l.lines[0].items[0].x, SETUP.left + 10.0);
        assert_eq!(l.lines[1].items[0].x, SETUP.left + 30.0);
        // A hang larger than the indent is clamped to the container edge.
        d.paragraphs[0].indent_left = Some(150);
        d.paragraphs[0].indent_first = Some(-3000);
        assert_eq!(lay(&d, SETUP, &m).lines[0].items[0].x, SETUP.left);
    }

    #[test]
    fn indent_and_alignment_combine_and_caret_follows() {
        let mut d = doc(&["ab"]);
        d.paragraphs[0].indent_left = Some(300);
        d.paragraphs[0].align = Align::Right;
        let l = lay(&d, SETUP, &m);
        assert_eq!(l.lines[0].items[0].x + 20.0, SETUP.left + 100.0);
        d.paragraphs[0].align = Align::Center;
        d.paragraphs[0].indent_first = Some(150);
        let l = lay(&d, SETUP, &m); // area: 10px first-line indent + 20px left → centered in remaining 70px
        assert_eq!(
            l.lines[0].items[0].x,
            SETUP.left + 20.0 + 10.0 + (70.0 - 20.0) / 2.0
        );
        assert_eq!(
            l.caret(Pos { para: 0, offset: 0 }, &m).x,
            l.lines[0].items[0].x
        );
    }

    #[test]
    fn table_cells_honor_paragraph_spacing() {
        let mut d = table_doc(&[&["a"], &["b"]]);
        for i in [1, 2] {
            d.paragraphs[i].space_after = Some(0);
        }
        let tight = lay(&d, WIDE, &m);
        let loose = lay(&table_doc(&[&["a"], &["b"]]), WIDE, &m);
        assert!(
            (loose.cells[0].h - tight.cells[0].h - PARA_SPACING).abs() < 1e-3
                || tight.cells[0].h <= loose.cells[0].h
        );
        assert!(
            (tight.cells[0].h - (LH + 2.0 * CELL_PAD)).abs() < 1e-3,
            "no trailing space inside the cell"
        );
    }

    // 200 wide, 50 margins (100px text), header/footer 10px from the edges.
    const HF: PageSetup = PageSetup {
        width: 200.0,
        height: 140.0,
        top: 50.0,
        right: 50.0,
        bottom: 50.0,
        left: 50.0,
        header: 10.0,
        footer: 10.0,
    };

    fn story_lines(l: &Layout, story: Story, page: usize) -> Vec<&Line> {
        l.lines
            .iter()
            .filter(|x| x.story == story && x.page == page)
            .collect()
    }

    fn with_footer(body: &[&str], footer: &str, align: Align) -> Document {
        let mut d = doc(body);
        let mut p = Paragraph::plain(footer);
        p.align = align;
        d.set_story_paras(Story::Footer, vec![p]);
        d
    }

    #[test]
    fn header_and_footer_repeat_on_every_page_at_their_distances() {
        let mut d = doc(&["a", "b", "c", "d", "e", "f"]);
        d.set_story_paras(Story::Header, vec![Paragraph::plain("head")]);
        d.set_story_paras(Story::Footer, vec![Paragraph::plain("foot")]);
        let l = layout(&d, HF, &m);
        assert!(l.pages >= 2);
        for page in 0..l.pages {
            let h = story_lines(&l, Story::Header, page);
            let f = story_lines(&l, Story::Footer, page);
            assert_eq!((h.len(), f.len()), (1, 1), "page {page}");
            assert_eq!(h[0].y, HF.header);
            assert!(
                (f[0].y + f[0].height - (HF.height - HF.footer)).abs() < 1e-3,
                "footer sits on its margin"
            );
            assert_eq!(h[0].items[0].text, "head");
            assert_eq!(h[0].items[0].x, HF.left);
        }
        // They don't take space from the body.
        let body_only = lay(&d, HF, &m);
        let plain = lay(&doc(&["a", "b", "c", "d", "e", "f"]), HF, &m);
        assert_eq!(body_only.pages, plain.pages);
    }

    #[test]
    fn empty_header_and_footer_still_have_clickable_lines() {
        let l = layout(&doc(&["a"]), HF, &m);
        assert_eq!(story_lines(&l, Story::Header, 0).len(), 1);
        assert_eq!(story_lines(&l, Story::Footer, 0).len(), 1);
        let mut focused = l.clone();
        focused.focus = Focus {
            story: Story::Footer,
            page: 0,
        };
        let p = focused.hit_test(0, 70.0, HF.height - 12.0, &m);
        assert_eq!(p, Pos::default());
        assert_eq!(focused.caret(Pos::default(), &m).page, 0);
    }

    #[test]
    fn page_number_fields_show_each_pages_number_and_the_total() {
        let mut d = doc(&["a", "b", "c", "d", "e", "f"]);
        let mut p = Paragraph::plain("");
        p.runs.push(quill_core::Run::new(
            format!("Page {PAGE_CHAR} of {}", quill_core::PAGES_CHAR),
            Format::default(),
        ));
        d.set_story_paras(Story::Footer, vec![p]);
        let l = layout(&d, HF, &m);
        for page in 0..l.pages {
            // The text may wrap at 10px per character; read every footer line.
            let text: String = story_lines(&l, Story::Footer, page)
                .iter()
                .flat_map(|f| f.items.iter())
                .map(|i| i.text.as_str())
                .collect();
            assert_eq!(
                text,
                format!("Page {} of {}", page + 1, l.pages),
                "page {page}"
            );
        }
    }

    #[test]
    fn a_field_counts_as_one_character_for_offsets_and_carets() {
        // Model text: "p" + PAGE + "q" → offsets 0..3, but the field shows "12".
        let mut d = doc(&["body"]);
        d.set_story_paras(
            Story::Footer,
            vec![Paragraph::plain(&format!("p{PAGE_CHAR}q"))],
        );
        let mut l = layout(&d, HF, &m);
        l.focus = Focus {
            story: Story::Footer,
            page: 0,
        };
        let line = &story_lines(&l, Story::Footer, 0)[0].clone();
        let field = line.items.iter().find(|i| i.atomic).unwrap();
        assert_eq!((field.text.as_str(), field.start), ("1", 1));
        let q = line.items.iter().find(|i| i.text == "q").unwrap();
        assert_eq!(q.start, 2, "the field advanced the offset by exactly one");
        let before = l.caret(Pos { para: 0, offset: 1 }, &m);
        let after = l.caret(Pos { para: 0, offset: 2 }, &m);
        assert_eq!(after.x - before.x, field.w);
        // Clicking either half of the field puts the caret before or after it, never inside.
        let y = before.y + 2.0;
        assert_eq!(l.hit_test(0, before.x + 1.0, y, &m).offset, 1);
        assert_eq!(l.hit_test(0, after.x - 1.0, y, &m).offset, 2);
        let r = l.selection_rects(Pos { para: 0, offset: 1 }, Pos { para: 0, offset: 2 }, &m);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].w, field.w);
    }

    #[test]
    fn lookups_follow_the_focused_story_and_page() {
        let mut d = doc(&["a", "b", "c", "d", "e", "f"]);
        d.set_story_paras(Story::Header, vec![Paragraph::plain("hdr")]);
        let mut l = layout(&d, HF, &m);
        assert!(l.pages >= 2);
        // Same paragraph index and offset exist in body, and in the header on every page.
        let pos = Pos { para: 0, offset: 1 };
        l.focus = Focus {
            story: Story::Body,
            page: 0,
        };
        let body_caret = l.caret(pos, &m);
        l.focus = Focus {
            story: Story::Header,
            page: 1,
        };
        let hdr_caret = l.caret(pos, &m);
        assert_eq!((hdr_caret.page, hdr_caret.y), (1, HF.header));
        assert_ne!(body_caret.y, hdr_caret.y);
        // Hit-testing only considers the focused story.
        let p = l.hit_test(1, 51.0, HF.header + 2.0, &m);
        assert_eq!((p.para, p.offset), (0, 0));
        l.focus = Focus {
            story: Story::Body,
            page: 0,
        };
        let body_hit = l.hit_test(0, 60.0, HF.header + 2.0, &m);
        assert_eq!(
            body_hit.para, 0,
            "snaps to the nearest body line, not the header"
        );
        // Up/Down never leave the focused story.
        l.focus = Focus {
            story: Story::Header,
            page: 0,
        };
        let up = l.vertical(Pos { para: 0, offset: 2 }, -1, 60.0, &m);
        let down = l.vertical(Pos { para: 0, offset: 2 }, 1, 60.0, &m);
        assert_eq!((up.para, down.para), (0, 0));
        // Selection rectangles are only produced for the focused page.
        let r = l.selection_rects(Pos { para: 0, offset: 0 }, Pos { para: 0, offset: 3 }, &m);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].page, 0);
    }

    #[test]
    fn header_paragraphs_do_not_confuse_body_paragraph_boundaries() {
        // A one-paragraph body whose header is also paragraph 0: the body's last line must
        // still be recognised as the last line (justify leaves it alone; clicks may go to its end).
        let mut d = doc(&["aaaa bbbb cccc"]);
        d.paragraphs[0].align = Align::Justify;
        d.set_story_paras(Story::Header, vec![Paragraph::plain("hdr")]);
        let l = layout(&d, SETUP, &m);
        let body: Vec<&Line> = l.lines.iter().filter(|x| x.story == Story::Body).collect();
        assert_eq!(body.len(), 2);
        assert_eq!(body[1].items[0].x, SETUP.left, "last line not justified");
        let mut l = l.clone();
        l.focus = Focus {
            story: Story::Body,
            page: 0,
        };
        assert_eq!(
            l.line_range(Pos {
                para: 0,
                offset: 12
            }),
            (10, 14)
        );
    }

    #[test]
    fn footer_alignment_and_multiple_lines_stack_upward() {
        let mut d = with_footer(&["a"], "f1", Align::Center);
        d.set_story_paras(
            Story::Footer,
            vec![
                {
                    let mut p = Paragraph::plain("one");
                    p.align = Align::Center;
                    p
                },
                Paragraph::plain("two"),
            ],
        );
        let l = layout(&d, HF, &m);
        let f = story_lines(&l, Story::Footer, 0);
        assert_eq!(f.len(), 2);
        assert!(f[1].y > f[0].y, "paragraphs run top to bottom");
        assert!(
            (f[1].y + f[1].height - (HF.height - HF.footer)).abs() < 1e-3,
            "last line on the margin"
        );
        assert_eq!(f[0].items[0].x, HF.left + 35.0, "centred: (100 - 30) / 2");
    }
}
