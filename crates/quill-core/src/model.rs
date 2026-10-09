//! Document model: paragraphs made of styled runs.
//!
//! Positions are `(paragraph index, char offset)`; offsets count Unicode
//! scalar values, not bytes.

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
}

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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paragraph {
    pub kind: Kind,
    pub runs: Vec<Run>,
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

    /// Format of the character before `offset` (or the first run at 0).
    pub fn format_at(&self, offset: usize) -> Format {
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

    /// Split into (before, after) at `offset`; both keep this paragraph's kind.
    fn split_at(&self, offset: usize) -> (Paragraph, Paragraph) {
        let mk = || Paragraph {
            kind: self.kind,
            runs: vec![],
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
}

impl Default for Document {
    fn default() -> Self {
        Self {
            paragraphs: vec![Paragraph::default()],
        }
    }
}

#[derive(PartialEq)]
enum CharClass {
    Word,
    Space,
    Other,
}

fn class(c: char) -> CharClass {
    if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else if c.is_whitespace() {
        CharClass::Space
    } else {
        CharClass::Other
    }
}

impl Document {
    pub fn from_paragraphs(mut paragraphs: Vec<Paragraph>) -> Self {
        if paragraphs.is_empty() {
            paragraphs.push(Paragraph::default());
        }
        for p in &mut paragraphs {
            p.normalize();
        }
        Self { paragraphs }
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
    pub fn delete_range(&mut self, start: Pos, end: Pos) -> Pos {
        let (start, end) = (self.clamp(start), self.clamp(end));
        if start >= end {
            return start;
        }
        let (mut a, _) = self.paragraphs[start.para].split_at(start.offset);
        let (_, c) = self.paragraphs[end.para].split_at(end.offset);
        a.runs.extend(c.runs);
        a.normalize();
        self.paragraphs.splice(start.para..=end.para, [a]);
        start
    }

    /// Delete the char before `pos`. At the start of a heading/list item this
    /// first demotes it to a normal paragraph; at the start of a normal one it
    /// joins with the previous paragraph.
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
        if pos.para == 0 {
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
        } else if pos.para + 1 < self.paragraphs.len() {
            Pos {
                para: pos.para + 1,
                offset: 0,
            }
        } else {
            return pos;
        };
        self.delete_range(pos, next)
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
