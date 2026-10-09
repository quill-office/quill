//! Paginated line layout: greedy word wrap over styled runs.
//!
//! Text measurement is injected (`Measure`) so the same code runs against a
//! browser canvas, a native shaper, or a fixed-width stub in tests.
//!
//! All coordinates are page-local: `x`/`y` include the page margin, and each
//! line records the page it sits on.

use quill_core::{Document, Format, Kind, Pos};
use serde::Serialize;

pub const BASE_SIZE: f32 = 17.0;
pub const LINE_FACTOR: f32 = 1.4;
pub const PARA_SPACING: f32 = 8.0;
pub const INDENT: f32 = 32.0;
const MARKER_GAP: f32 = 8.0;
const PARA_MARK_WIDTH: f32 = 6.0;

#[derive(Clone, Copy, Debug)]
pub struct TextStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub size: f32,
}

pub trait Measure {
    fn width(&self, text: &str, style: &TextStyle) -> f32;
}

impl<F: Fn(&str, &TextStyle) -> f32> Measure for F {
    fn width(&self, text: &str, style: &TextStyle) -> f32 {
        self(text, style)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PageSetup {
    pub width: f32,
    pub height: f32,
    pub margin: f32,
}

impl PageSetup {
    pub const LETTER: PageSetup = PageSetup {
        width: 816.0,
        height: 1056.0,
        margin: 72.0,
    };
}

pub fn para_size(kind: Kind) -> f32 {
    match kind {
        Kind::Heading(1) => 28.0,
        Kind::Heading(2) => 22.0,
        Kind::Heading(_) => 19.0,
        _ => BASE_SIZE,
    }
}

fn style_for(kind: Kind, f: &Format) -> TextStyle {
    TextStyle {
        bold: f.bold || matches!(kind, Kind::Heading(_)),
        italic: f.italic,
        underline: f.underline,
        size: para_size(kind),
    }
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
    /// Char offset of `text` within its paragraph.
    pub start: usize,
}

impl Item {
    fn style(&self) -> TextStyle {
        TextStyle {
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            size: self.size,
        }
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Marker {
    pub text: String,
    pub x: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Line {
    pub para: usize,
    pub page: usize,
    /// Left edge of the text area (after list indent).
    pub x: f32,
    pub y: f32,
    pub height: f32,
    /// Distance from `y` to the text baseline.
    pub baseline: f32,
    pub items: Vec<Item>,
    /// Paragraph char range covered by this line.
    pub start: usize,
    pub end: usize,
    pub marker: Option<Marker>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Layout {
    pub lines: Vec<Line>,
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
    page: usize,
    /// Cursor within the page content area.
    y: f32,
    margin: f32,
    content_h: f32,
}

impl Pager {
    fn push(&mut self, mut line: Line) {
        if self.y + line.height > self.content_h && self.y > 0.0 {
            self.page += 1;
            self.y = 0.0;
        }
        line.page = self.page;
        line.y = self.margin + self.y;
        self.y += line.height;
        self.lines.push(line);
    }
}

pub fn layout(doc: &Document, setup: PageSetup, m: &dyn Measure) -> Layout {
    let content_w = setup.width - 2.0 * setup.margin;
    let mut pager = Pager {
        lines: vec![],
        page: 0,
        y: 0.0,
        margin: setup.margin,
        content_h: setup.height - 2.0 * setup.margin,
    };
    let mut number = 0usize;

    for (pi, para) in doc.paragraphs.iter().enumerate() {
        number = if para.kind == Kind::Numbered {
            number + 1
        } else {
            0
        };
        let size = para_size(para.kind);
        let height = size * LINE_FACTOR;
        let indent = if para.kind.is_list() { INDENT } else { 0.0 };
        let avail = content_w - indent;
        let x0 = setup.margin + indent;
        if matches!(para.kind, Kind::Heading(_)) && pager.y > 0.0 {
            pager.y += size * 0.4;
        }

        let marker = match para.kind {
            Kind::Bullet | Kind::Numbered => {
                let text = if para.kind == Kind::Bullet {
                    "•".to_string()
                } else {
                    format!("{number}.")
                };
                let st = style_for(para.kind, &Format::default());
                let x = x0 - MARKER_GAP - m.width(&text, &st);
                Some(Marker { text, x })
            }
            _ => None,
        };
        let new_line = |start: usize, marker: Option<Marker>| Line {
            para: pi,
            page: 0,
            x: x0,
            y: 0.0,
            height,
            baseline: size,
            items: vec![],
            start,
            end: start,
            marker,
        };

        let mut cur = new_line(0, marker);
        let (mut x, mut offset) = (0.0f32, 0usize);
        for run in &para.runs {
            let st = style_for(para.kind, &run.format);
            let item = |x: f32, text: &str, start: usize| Item {
                x: x0 + x,
                text: text.to_string(),
                bold: st.bold,
                italic: st.italic,
                underline: st.underline,
                size: st.size,
                start,
            };
            for tok in tokens(&run.text) {
                let n = tok.chars().count();
                let w = m.width(tok, &st);
                if x + w > avail && !cur.items.is_empty() {
                    cur.end = offset;
                    pager.push(std::mem::replace(&mut cur, new_line(offset, None)));
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
                        cur.items.push(item(x, &chunk, offset + i));
                        x += m.width(&chunk, &st);
                        i = j;
                        if i < chars.len() {
                            cur.end = offset + i;
                            pager.push(std::mem::replace(&mut cur, new_line(offset + i, None)));
                            x = 0.0;
                        }
                    }
                } else {
                    cur.items.push(item(x, tok, offset));
                    x += w;
                }
                offset += n;
                if tok.ends_with('\n') {
                    cur.end = offset;
                    pager.push(std::mem::replace(&mut cur, new_line(offset, None)));
                    x = 0.0;
                }
            }
        }
        cur.end = offset;
        pager.push(cur);
        pager.y += PARA_SPACING;
    }
    let pages = pager.page + 1;
    Layout {
        lines: pager.lines,
        pages,
    }
}

impl Layout {
    fn is_last_line(&self, li: usize) -> bool {
        self.lines
            .get(li + 1)
            .is_none_or(|n| n.para != self.lines[li].para)
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
        self.lines
            .iter()
            .rposition(|l| l.para == pos.para && l.start <= pos.offset)
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
                let prefix: String = it.text.chars().take(offset - it.start).collect();
                x = it.x + m.width(&prefix, &it.style());
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
            let st = it.style();
            let chars: Vec<char> = it.text.chars().collect();
            let mut prev = 0.0;
            for k in 1..=chars.len() {
                let s: String = chars[..k].iter().collect();
                let w = m.width(&s, &st);
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

    /// Map a page-local point to a document position.
    pub fn hit_test(&self, page: usize, x: f32, y: f32, m: &dyn Measure) -> Pos {
        let on_page: Vec<usize> = (0..self.lines.len())
            .filter(|&i| self.lines[i].page == page)
            .collect();
        let li = match on_page.iter().rev().find(|&&i| self.lines[i].y <= y) {
            Some(&i) => i,
            None => on_page.first().copied().unwrap_or(self.lines.len() - 1),
        };
        self.hit_line(li, x, m)
    }

    /// Caret one visual line up (`dir < 0`) or down, keeping horizontal `goal_x`.
    pub fn vertical(&self, pos: Pos, dir: i32, goal_x: f32, m: &dyn Measure) -> Pos {
        let li = self.line_index(pos) as i64 + dir.signum() as i64;
        let li = li.clamp(0, self.lines.len() as i64 - 1) as usize;
        let at_edge = li == self.line_index(pos);
        if at_edge {
            // Already on the first/last line: jump to its start/end.
            let x = if dir < 0 { f32::MIN } else { f32::MAX };
            return self.hit_line(li, x, m);
        }
        self.hit_line(li, goal_x, m)
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
            if l.para < start.para || l.para > end.para {
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
    use quill_core::Paragraph;

    // 10px per char; ignores style.
    fn m(t: &str, _: &TextStyle) -> f32 {
        t.chars().count() as f32 * 10.0
    }

    // 100px content width, 40px margins, 4 lines (of 23.8px) per page.
    const SETUP: PageSetup = PageSetup {
        width: 180.0,
        height: 80.0 + 4.0 * 23.8 + 1.0,
        margin: 40.0,
    };

    fn doc(texts: &[&str]) -> Document {
        Document::from_paragraphs(texts.iter().map(|t| Paragraph::plain(t)).collect())
    }

    #[test]
    fn wraps_at_width() {
        let d = doc(&["aaaa bbbb cccc"]);
        let l = layout(&d, SETUP, &m);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[1].items[0].text, "cccc");
        assert_eq!(l.lines[1].start, 10);
    }

    #[test]
    fn empty_paragraph_has_a_line() {
        assert_eq!(layout(&Document::default(), SETUP, &m).lines.len(), 1);
    }

    #[test]
    fn long_word_is_broken_by_chars() {
        let d = doc(&["abcdefghijklmnopqrstuvwxyz"]);
        let l = layout(&d, SETUP, &m);
        let texts: Vec<_> = l.lines.iter().map(|l| l.items[0].text.as_str()).collect();
        assert_eq!(texts, ["abcdefghij", "klmnopqrst", "uvwxyz"]);
        assert_eq!(l.lines[2].start, 20);
    }

    #[test]
    fn forced_newline_breaks_line() {
        let mut d = doc(&["ab"]);
        d.paragraphs[0].runs[0].text = "ab\ncd".into();
        let l = layout(&d, SETUP, &m);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[1].start, 3);
    }

    #[test]
    fn paginates() {
        let d = doc(&["a", "b", "c", "d", "e"]);
        let l = layout(&d, SETUP, &m);
        assert!(l.pages >= 2, "pages = {}", l.pages);
        assert_eq!(l.lines[0].page, 0);
        assert_eq!(l.lines.last().unwrap().page, l.pages - 1);
        let first_on_p1 = l.lines.iter().find(|l| l.page == 1).unwrap();
        assert_eq!(
            first_on_p1.y, SETUP.margin,
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
        let l = layout(&d, SETUP, &m);
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
        assert_eq!(l.lines[0].items[0].x, SETUP.margin + INDENT);
    }

    #[test]
    fn heading_is_larger_and_bold() {
        let d = Document::from_paragraphs(vec![Paragraph::plain("T").with_kind(Kind::Heading(1))]);
        let l = layout(&d, SETUP, &m);
        assert_eq!(l.lines[0].items[0].size, 28.0);
        assert!(l.lines[0].items[0].bold);
        assert!(l.lines[0].height > BASE_SIZE * LINE_FACTOR);
    }

    #[test]
    fn caret_at_wrap_point_is_on_next_line() {
        let d = doc(&["aaaa bbbb cccc"]);
        let l = layout(&d, SETUP, &m);
        let c = l.caret(
            Pos {
                para: 0,
                offset: 10,
            },
            &m,
        );
        assert_eq!((c.x, c.y), (SETUP.margin, SETUP.margin + 23.8));
    }

    #[test]
    fn hit_test_round_trips_caret_across_pages() {
        let d = doc(&["aaaa bbbb cccc", "xyz", "q", "r", "s"]);
        let l = layout(&d, SETUP, &m);
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
        let l = layout(&d, SETUP, &m);
        let p = l.hit_test(0, 500.0, SETUP.margin + 1.0, &m);
        assert_eq!(p, Pos { para: 0, offset: 9 }, "before the soft-wrap space");
    }

    #[test]
    fn vertical_keeps_goal_x() {
        let d = doc(&["aaaa bbbb cccc", "xy"]);
        let l = layout(&d, SETUP, &m);
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
        let l = layout(&d, SETUP, &m);
        let r = l.selection_rects(Pos { para: 0, offset: 2 }, Pos { para: 1, offset: 1 }, &m);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].x, SETUP.margin + 20.0);
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
        let l = layout(&d, SETUP, &m);
        assert_eq!(l.line_range(Pos { para: 0, offset: 2 }), (0, 9));
        assert_eq!(
            l.line_range(Pos {
                para: 0,
                offset: 12
            }),
            (10, 14)
        );
    }
}
