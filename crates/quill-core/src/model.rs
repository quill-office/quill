//! Document model: paragraphs made of styled runs.
//!
//! Positions are `(paragraph index, char offset)`; offsets count Unicode
//! scalar values, not bytes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attr {
    Bold,
    Italic,
    Underline,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Format {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Index into `Document::images`. Set only on runs made of `OBJECT_CHAR`s.
    #[serde(default)]
    pub image: Option<u32>,
    /// Font size in half-points (as in `.docx`); `None` follows the paragraph style.
    #[serde(default)]
    pub size: Option<u16>,
    /// Font family name; `None` is the document default.
    #[serde(default)]
    pub font: Option<Arc<str>>,
    /// Text colour as `0xRRGGBB`; `None` is the default text colour.
    #[serde(default)]
    pub color: Option<u32>,
}

/// Placeholder character standing for one inline image in paragraph text.
pub const OBJECT_CHAR: char = '\u{FFFC}';

static NEXT_UID: AtomicU64 = AtomicU64::new(1);

/// An embedded raster image. `width`/`height` are the display size in CSS px.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Image {
    /// Identifies the pixel data, so hosts can cache decoded images. Clones
    /// (and resized copies) share it.
    pub uid: u64,
    pub mime: String,
    pub data: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}

impl Image {
    pub fn new(mime: impl Into<String>, data: Vec<u8>, width: u32, height: u32) -> Self {
        Self {
            uid: NEXT_UID.fetch_add(1, Ordering::Relaxed),
            mime: mime.into(),
            data: data.into(),
            width: width.max(1),
            height: height.max(1),
        }
    }
}

impl PartialEq for Image {
    fn eq(&self, o: &Self) -> bool {
        self.width == o.width
            && self.height == o.height
            && self.mime == o.mime
            && (Arc::ptr_eq(&self.data, &o.data) || self.data == o.data)
    }
}

impl Eq for Image {}

impl Format {
    pub fn get(&self, a: Attr) -> bool {
        match a {
            Attr::Bold => self.bold,
            Attr::Italic => self.italic,
            Attr::Underline => self.underline,
        }
    }

    pub fn set(&mut self, a: Attr, v: bool) {
        match a {
            Attr::Bold => self.bold = v,
            Attr::Italic => self.italic = v,
            Attr::Underline => self.underline = v,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    #[default]
    Normal,
    /// Level 1..=3.
    Heading(u8),
    Bullet,
    Numbered,
}

impl Kind {
    pub fn is_list(self) -> bool {
        matches!(self, Kind::Bullet | Kind::Numbered)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub text: String,
    pub format: Format,
}

impl Run {
    pub fn new(text: impl Into<String>, format: Format) -> Self {
        Self {
            text: text.into(),
            format,
        }
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }
}

/// A table as rows → columns → the paragraphs of each cell.
type Grid = Vec<Vec<Vec<Paragraph>>>;

/// Which table cell a paragraph lives in. A table is a run of consecutive
/// paragraphs sharing `table`, stored in row-major order; a cell may hold
/// several paragraphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub table: u32,
    pub row: u16,
    pub col: u16,
}

impl Cell {
    /// True if both cells are the same cell of the same table.
    fn same_cell(a: Option<Cell>, b: Option<Cell>) -> bool {
        a == b
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paragraph {
    pub kind: Kind,
    #[serde(default)]
    pub align: Align,
    pub runs: Vec<Run>,
    #[serde(default)]
    pub cell: Option<Cell>,
}

impl Paragraph {
    pub fn plain(text: &str) -> Self {
        let mut p = Paragraph::default();
        if !text.is_empty() {
            p.runs.push(Run::new(text, Format::default()));
        }
        p
    }

    pub fn with_kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }

    pub fn len(&self) -> usize {
        self.runs.iter().map(Run::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    /// Format of the character before `offset` (or the first run at 0). Never
    /// carries an image, so typing after an image yields text.
    pub fn format_at(&self, offset: usize) -> Format {
        Format {
            image: None,
            ..self.raw_format_at(offset)
        }
    }

    fn raw_format_at(&self, offset: usize) -> Format {
        let mut pos = 0;
        let mut last = None;
        for r in &self.runs {
            let end = pos + r.len();
            if offset <= end && (offset > pos || last.is_none()) {
                return r.format.clone();
            }
            pos = end;
            last = Some(&r.format);
        }
        last.cloned().unwrap_or_default()
    }

    /// Split into (before, after) at `offset`; both keep this paragraph's kind, alignment and cell.
    fn split_at(&self, offset: usize) -> (Paragraph, Paragraph) {
        let mk = || Paragraph {
            kind: self.kind,
            align: self.align,
            runs: vec![],
            cell: self.cell,
        };
        let (mut a, mut b) = (mk(), mk());
        let mut pos = 0;
        for r in &self.runs {
            let n = r.len();
            if pos + n <= offset {
                a.runs.push(r.clone());
            } else if pos >= offset {
                b.runs.push(r.clone());
            } else {
                let k = offset - pos;
                let head: String = r.text.chars().take(k).collect();
                let tail: String = r.text.chars().skip(k).collect();
                a.runs.push(Run::new(head, r.format.clone()));
                b.runs.push(Run::new(tail, r.format.clone()));
            }
            pos += n;
        }
        (a, b)
    }

    /// Drop empty runs and merge neighbours that share a format.
    pub fn normalize(&mut self) {
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        for r in self.runs.drain(..) {
            if r.text.is_empty() {
                continue;
            }
            match out.last_mut() {
                Some(l) if l.format == r.format => l.text.push_str(&r.text),
                _ => out.push(r),
            }
        }
        self.runs = out;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pos {
    pub para: usize,
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub paragraphs: Vec<Paragraph>,
    #[serde(default)]
    pub images: Vec<Image>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            paragraphs: vec![Paragraph::default()],
            images: vec![],
        }
    }
}

#[derive(PartialEq)]
enum CharClass {
    Word,
    Space,
    Other,
    Object,
}

fn class(c: char) -> CharClass {
    if c == OBJECT_CHAR {
        CharClass::Object
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else if c.is_whitespace() {
        CharClass::Space
    } else {
        CharClass::Other
    }
}

impl Document {
    pub fn from_paragraphs(mut paragraphs: Vec<Paragraph>) -> Self {
        for p in &mut paragraphs {
            p.normalize();
        }
        let mut paragraphs = repair_tables(paragraphs);
        if paragraphs.is_empty() {
            paragraphs.push(Paragraph::default());
        }
        Self {
            paragraphs,
            images: vec![],
        }
    }

    pub fn plain_text(&self) -> String {
        self.paragraphs
            .iter()
            .map(Paragraph::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn end(&self) -> Pos {
        let para = self.paragraphs.len() - 1;
        Pos {
            para,
            offset: self.paragraphs[para].len(),
        }
    }

    pub fn clamp(&self, pos: Pos) -> Pos {
        let para = pos.para.min(self.paragraphs.len() - 1);
        Pos {
            para,
            offset: pos.offset.min(self.paragraphs[para].len()),
        }
    }

    /// Format that text typed at `pos` would inherit.
    pub fn format_at(&self, pos: Pos) -> Format {
        let pos = self.clamp(pos);
        self.paragraphs[pos.para].format_at(pos.offset)
    }

    /// Insert `text` at `pos`; newlines start new paragraphs. `fmt` overrides
    /// the inherited format. Returns the new cursor.
    pub fn insert_text(&mut self, pos: Pos, text: &str, fmt: Option<&Format>) -> Pos {
        let mut pos = self.clamp(pos);
        for (i, piece) in text.split('\n').enumerate() {
            if i > 0 {
                pos = self.split_paragraph(pos);
            }
            pos = self.insert_inline(pos, piece, fmt);
        }
        pos
    }

    fn insert_inline(&mut self, pos: Pos, text: &str, fmt: Option<&Format>) -> Pos {
        if text.is_empty() {
            return pos;
        }
        let p = &mut self.paragraphs[pos.para];
        let fmt = fmt.cloned().unwrap_or_else(|| p.format_at(pos.offset));
        let (mut a, b) = p.split_at(pos.offset);
        a.runs.push(Run::new(text, fmt));
        a.runs.extend(b.runs);
        a.normalize();
        *p = a;
        Pos {
            para: pos.para,
            offset: pos.offset + text.chars().count(),
        }
    }

    /// Split at `pos`; both halves keep the kind.
    pub fn split_paragraph(&mut self, pos: Pos) -> Pos {
        let pos = self.clamp(pos);
        let (mut a, mut b) = self.paragraphs[pos.para].split_at(pos.offset);
        a.normalize();
        b.normalize();
        self.paragraphs[pos.para] = a;
        self.paragraphs.insert(pos.para + 1, b);
        Pos {
            para: pos.para + 1,
            offset: 0,
        }
    }

    /// Delete `[start, end)`; the merged paragraph keeps `start`'s kind.
    ///
    /// Table structure is never destroyed by a text deletion: paragraphs that
    /// belong to table cells are emptied rather than removed, and paragraphs
    /// are only merged when nothing structural lies between them.
    pub fn delete_range(&mut self, start: Pos, end: Pos) -> Pos {
        let (start, end) = (self.clamp(start), self.clamp(end));
        if start >= end {
            return start;
        }
        if start.para == end.para {
            let p = &mut self.paragraphs[start.para];
            let (mut a, b) = p.split_at(start.offset);
            let (_, c) = b.split_at(end.offset - start.offset);
            a.runs.extend(c.runs);
            a.normalize();
            *p = a;
            return start;
        }
        let (mut head, _) = self.paragraphs[start.para].split_at(start.offset);
        let (_, tail) = self.paragraphs[end.para].split_at(end.offset);
        head.normalize();
        let mut tail = tail;
        tail.normalize();
        // Interior paragraphs: drop plain ones, empty table ones.
        let mut keep: Vec<Paragraph> = Vec::new();
        for i in start.para + 1..end.para {
            let p = &self.paragraphs[i];
            if p.cell.is_some() {
                keep.push(Paragraph {
                    cell: p.cell,
                    ..Default::default()
                });
            }
        }
        let adjacent = keep.is_empty();
        let mut replacement = Vec::with_capacity(keep.len() + 2);
        if adjacent && Cell::same_cell(head.cell, tail.cell) {
            head.runs.extend(tail.runs);
            head.normalize();
            replacement.push(head);
        } else {
            replacement.push(head);
            replacement.extend(keep);
            replacement.push(tail);
        }
        self.paragraphs.splice(start.para..=end.para, replacement);
        start
    }

    /// Delete the char before `pos`. At the start of a heading/list item this
    /// first demotes it to a normal paragraph; at the start of a normal one it
    /// joins with the previous paragraph (unless that crosses a table edge).
    pub fn backspace(&mut self, pos: Pos) -> Pos {
        let pos = self.clamp(pos);
        if pos.offset > 0 {
            return self.delete_range(
                Pos {
                    para: pos.para,
                    offset: pos.offset - 1,
                },
                pos,
            );
        }
        let p = &mut self.paragraphs[pos.para];
        if p.kind != Kind::Normal {
            p.kind = Kind::Normal;
            return pos;
        }
        if pos.para == 0
            || !Cell::same_cell(
                self.paragraphs[pos.para - 1].cell,
                self.paragraphs[pos.para].cell,
            )
        {
            return pos;
        }
        let prev_end = Pos {
            para: pos.para - 1,
            offset: self.paragraphs[pos.para - 1].len(),
        };
        self.delete_range(prev_end, pos)
    }

    pub fn delete_forward(&mut self, pos: Pos) -> Pos {
        let pos = self.clamp(pos);
        let next = if pos.offset < self.paragraphs[pos.para].len() {
            Pos {
                para: pos.para,
                offset: pos.offset + 1,
            }
        } else if pos.para + 1 < self.paragraphs.len()
            && Cell::same_cell(
                self.paragraphs[pos.para].cell,
                self.paragraphs[pos.para + 1].cell,
            )
        {
            Pos {
                para: pos.para + 1,
                offset: 0,
            }
        } else {
            return pos;
        };
        self.delete_range(pos, next)
    }

    // ---- tables ----------------------------------------------------------

    pub fn cell_at(&self, para: usize) -> Option<Cell> {
        self.paragraphs.get(para).and_then(|p| p.cell)
    }

    /// First position of the given cell, if it exists.
    pub fn cell_start(&self, table: u32, row: u16, col: u16) -> Option<Pos> {
        let want = Cell { table, row, col };
        self.paragraphs
            .iter()
            .position(|p| p.cell == Some(want))
            .map(|para| Pos { para, offset: 0 })
    }

    /// Number of (rows, cols) of a table.
    pub fn table_dims(&self, table: u32) -> Option<(usize, usize)> {
        let cells: Vec<Cell> = self
            .paragraphs
            .iter()
            .filter_map(|p| p.cell)
            .filter(|c| c.table == table)
            .collect();
        let rows = cells.iter().map(|c| c.row as usize + 1).max()?;
        let cols = cells.iter().map(|c| c.col as usize + 1).max()?;
        Some((rows, cols))
    }

    /// Insert an empty `rows` × `cols` table at `pos` (splitting the
    /// paragraph there). Returns the start of the first cell, or `None` when
    /// `pos` is already inside a table (no nesting) or the size is zero.
    pub fn insert_table(&mut self, pos: Pos, rows: usize, cols: usize) -> Option<Pos> {
        let pos = self.clamp(pos);
        if rows == 0
            || cols == 0
            || rows > u16::MAX as usize
            || cols > u16::MAX as usize
            || self.paragraphs[pos.para].cell.is_some()
        {
            return None;
        }
        let after = self.split_paragraph(pos);
        if self.paragraphs[after.para].is_empty() {
            self.paragraphs[after.para].kind = Kind::Normal;
        }
        let id = self
            .paragraphs
            .iter()
            .filter_map(|p| p.cell)
            .map(|c| c.table)
            .max()
            .map_or(0, |m| m + 1);
        let mut cells = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            for c in 0..cols {
                cells.push(Paragraph {
                    cell: Some(Cell {
                        table: id,
                        row: r as u16,
                        col: c as u16,
                    }),
                    ..Default::default()
                });
            }
        }
        self.paragraphs.splice(after.para..after.para, cells);
        Some(Pos {
            para: after.para,
            offset: 0,
        })
    }

    /// Extract a table as rows → cols → paragraphs, with its index range.
    fn take_grid(&self, table: u32) -> Option<(usize, usize, Grid)> {
        let first = self
            .paragraphs
            .iter()
            .position(|p| p.cell.is_some_and(|c| c.table == table))?;
        let last = self
            .paragraphs
            .iter()
            .rposition(|p| p.cell.is_some_and(|c| c.table == table))?;
        let (rows, cols) = self.table_dims(table)?;
        let mut grid = vec![vec![Vec::new(); cols]; rows];
        for p in &self.paragraphs[first..=last] {
            let c = p.cell?;
            grid[c.row as usize][c.col as usize].push(p.clone());
        }
        Some((first, last, grid))
    }

    fn put_grid(&mut self, table: u32, first: usize, last: usize, grid: Grid) {
        let mut flat = Vec::new();
        for (r, row) in grid.into_iter().enumerate() {
            for (c, mut cell) in row.into_iter().enumerate() {
                if cell.is_empty() {
                    cell.push(Paragraph::default());
                }
                for mut p in cell {
                    p.cell = Some(Cell {
                        table,
                        row: r as u16,
                        col: c as u16,
                    });
                    flat.push(p);
                }
            }
        }
        self.paragraphs.splice(first..=last, flat);
    }

    /// Run `f` on the grid of the table containing `para`; returns where the
    /// caret should go afterwards (a cell start, or the paragraph where the
    /// table was removed).
    fn edit_table(
        &mut self,
        para: usize,
        f: impl FnOnce(&mut Grid, Cell) -> (u16, u16),
    ) -> Option<Pos> {
        let cell = self.cell_at(para)?;
        let (first, last, mut grid) = self.take_grid(cell.table)?;
        let (row, col) = f(&mut grid, cell);
        if grid.is_empty() || grid[0].is_empty() {
            self.paragraphs.splice(first..=last, std::iter::empty());
            if self.paragraphs.is_empty() {
                self.paragraphs.push(Paragraph::default());
            }
            return Some(Pos {
                para: first.min(self.paragraphs.len() - 1),
                offset: 0,
            });
        }
        self.put_grid(cell.table, first, last, grid);
        self.cell_start(cell.table, row, col)
    }

    /// Insert an empty row below the row containing `para`.
    pub fn add_row(&mut self, para: usize) -> Option<Pos> {
        self.edit_table(para, |g, c| {
            let cols = g[0].len();
            g.insert(c.row as usize + 1, vec![Vec::new(); cols]);
            (c.row + 1, c.col)
        })
    }

    /// Insert an empty column right of the column containing `para`.
    pub fn add_col(&mut self, para: usize) -> Option<Pos> {
        self.edit_table(para, |g, c| {
            for row in g.iter_mut() {
                row.insert(c.col as usize + 1, Vec::new());
            }
            (c.row, c.col + 1)
        })
    }

    /// Remove the row containing `para` (the whole table if it is the last row).
    pub fn delete_row(&mut self, para: usize) -> Option<Pos> {
        self.edit_table(para, |g, c| {
            g.remove(c.row as usize);
            (c.row.min(g.len().saturating_sub(1) as u16), c.col)
        })
    }

    /// Remove the column containing `para` (the whole table if it is the last column).
    pub fn delete_col(&mut self, para: usize) -> Option<Pos> {
        self.edit_table(para, |g, c| {
            for row in g.iter_mut() {
                row.remove(c.col as usize);
            }
            (c.row, c.col.min(g[0].len().saturating_sub(1) as u16))
        })
    }

    pub fn delete_table(&mut self, para: usize) -> Option<Pos> {
        self.edit_table(para, |g, _| {
            g.clear();
            (0, 0)
        })
    }

    /// Insert an inline image at `pos`; returns the position after it.
    pub fn insert_image(&mut self, pos: Pos, image: Image) -> Pos {
        let id = self.images.len() as u32;
        self.images.push(image);
        let fmt = Format {
            image: Some(id),
            ..Format::default()
        };
        self.insert_text(pos, &OBJECT_CHAR.to_string(), Some(&fmt))
    }

    /// Ids of the images inside `[start, end)`, in order, without repeats.
    pub fn images_in_range(&self, start: Pos, end: Pos) -> Vec<u32> {
        let (start, end) = (self.clamp(start), self.clamp(end));
        let mut out = Vec::new();
        if start >= end {
            return out;
        }
        for pi in start.para..=end.para {
            let s = if pi == start.para { start.offset } else { 0 };
            let e = if pi == end.para {
                end.offset
            } else {
                usize::MAX
            };
            let mut pos = 0;
            for r in &self.paragraphs[pi].runs {
                let n = r.len();
                if let Some(id) = r.format.image {
                    if pos.max(s) < (pos + n).min(e) && !out.contains(&id) {
                        out.push(id);
                    }
                }
                pos += n;
            }
        }
        out
    }

    /// Multiply the display size of the given images (clamped to 16..=4000 px).
    pub fn scale_images(&mut self, ids: &[u32], factor: f32) {
        let scale = |v: u32| ((v as f32 * factor).round() as u32).clamp(16, 4000);
        for &id in ids {
            if let Some(img) = self.images.get_mut(id as usize) {
                img.width = scale(img.width);
                img.height = scale(img.height);
            }
        }
    }

    pub fn text_in_range(&self, start: Pos, end: Pos) -> String {
        let (start, end) = (self.clamp(start), self.clamp(end));
        let mut out = String::new();
        for pi in start.para..=end.para.max(start.para) {
            if pi > start.para {
                out.push('\n');
            }
            let s = if pi == start.para { start.offset } else { 0 };
            let e = if pi == end.para {
                end.offset
            } else {
                usize::MAX
            };
            out.extend(
                self.paragraphs[pi]
                    .text()
                    .chars()
                    .skip(s)
                    .take(e.saturating_sub(s)),
            );
        }
        out
    }

    /// Apply `f` to the format of every char in `[start, end)` of one paragraph.
    pub fn set_format(&mut self, para: usize, start: usize, end: usize, f: impl Fn(&mut Format)) {
        let p = &mut self.paragraphs[para];
        let (a, rest) = p.split_at(start);
        let (mut mid, c) = rest.split_at(end.saturating_sub(start));
        for r in &mut mid.runs {
            f(&mut r.format);
        }
        let mut out = a;
        out.runs.extend(mid.runs);
        out.runs.extend(c.runs);
        out.normalize();
        *p = out;
    }

    pub fn set_format_range(&mut self, start: Pos, end: Pos, f: impl Fn(&mut Format)) {
        let (start, end) = (self.clamp(start), self.clamp(end));
        for pi in start.para..=end.para {
            let s = if pi == start.para { start.offset } else { 0 };
            let e = if pi == end.para {
                end.offset
            } else {
                self.paragraphs[pi].len()
            };
            if s < e {
                self.set_format(pi, s, e, &f);
            }
        }
    }

    /// True if the range is non-empty and every char has attribute `a` set.
    pub fn range_all(&self, start: Pos, end: Pos, a: Attr) -> bool {
        let (start, end) = (self.clamp(start), self.clamp(end));
        let mut seen = false;
        for pi in start.para..=end.para {
            let s = if pi == start.para { start.offset } else { 0 };
            let e = if pi == end.para {
                end.offset
            } else {
                usize::MAX
            };
            let mut pos = 0;
            for r in &self.paragraphs[pi].runs {
                let n = r.len();
                if pos.max(s) < (pos + n).min(e) {
                    seen = true;
                    if !r.format.get(a) {
                        return false;
                    }
                }
                pos += n;
            }
        }
        seen
    }

    /// Word (or whitespace/punctuation run) around `pos`.
    pub fn word_range(&self, pos: Pos) -> (Pos, Pos) {
        let pos = self.clamp(pos);
        let chars: Vec<char> = self.paragraphs[pos.para].text().chars().collect();
        if chars.is_empty() {
            return (pos, pos);
        }
        let at = pos.offset.min(chars.len() - 1);
        let k = class(chars[at]);
        if k == CharClass::Object {
            return (
                Pos {
                    para: pos.para,
                    offset: at,
                },
                Pos {
                    para: pos.para,
                    offset: at + 1,
                },
            );
        }
        let mut s = at;
        while s > 0 && class(chars[s - 1]) == k {
            s -= 1;
        }
        let mut e = at + 1;
        while e < chars.len() && class(chars[e]) == k {
            e += 1;
        }
        (
            Pos {
                para: pos.para,
                offset: s,
            },
            Pos {
                para: pos.para,
                offset: e,
            },
        )
    }

    pub fn move_left(&self, pos: Pos) -> Pos {
        let pos = self.clamp(pos);
        if pos.offset > 0 {
            Pos {
                offset: pos.offset - 1,
                ..pos
            }
        } else if pos.para > 0 {
            Pos {
                para: pos.para - 1,
                offset: self.paragraphs[pos.para - 1].len(),
            }
        } else {
            pos
        }
    }

    pub fn move_right(&self, pos: Pos) -> Pos {
        let pos = self.clamp(pos);
        if pos.offset < self.paragraphs[pos.para].len() {
            Pos {
                offset: pos.offset + 1,
                ..pos
            }
        } else if pos.para + 1 < self.paragraphs.len() {
            Pos {
                para: pos.para + 1,
                offset: 0,
            }
        } else {
            pos
        }
    }
}

/// Make table paragraphs well-formed: contiguous rows × cols grids in
/// row-major order, no gaps, and a non-table paragraph after every table.
fn repair_tables(paras: Vec<Paragraph>) -> Vec<Paragraph> {
    use std::collections::BTreeMap;
    let mut out = Vec::with_capacity(paras.len());
    let mut i = 0;
    while i < paras.len() {
        let Some(first) = paras[i].cell else {
            out.push(paras[i].clone());
            i += 1;
            continue;
        };
        let mut j = i;
        while j < paras.len() && paras[j].cell.is_some_and(|c| c.table == first.table) {
            j += 1;
        }
        let mut cells: BTreeMap<(u16, u16), Vec<Paragraph>> = BTreeMap::new();
        for p in &paras[i..j] {
            let c = p.cell.expect("checked above");
            cells.entry((c.row, c.col)).or_default().push(p.clone());
        }
        let mut rows: Vec<u16> = cells.keys().map(|k| k.0).collect();
        rows.dedup();
        let cols = cells.keys().map(|k| k.1 as usize + 1).max().unwrap_or(1);
        for (ri, row) in rows.iter().enumerate() {
            for col in 0..cols {
                let mut cell = cells.remove(&(*row, col as u16)).unwrap_or_default();
                if cell.is_empty() {
                    cell.push(Paragraph::default());
                }
                for mut p in cell {
                    p.cell = Some(Cell {
                        table: first.table,
                        row: ri as u16,
                        col: col as u16,
                    });
                    out.push(p);
                }
            }
        }
        if j >= paras.len() || paras[j].cell.is_some() {
            out.push(Paragraph::default());
        }
        i = j;
    }
    out
}
