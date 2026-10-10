//! Editing session: selection, pending format, and undo/redo history.
//!
//! History is snapshot-based with coalescing of consecutive typing/deleting.
//! (A merge-friendly operation log arrives with collaboration.)

use crate::find::FindOptions;
use crate::model::*;

const MAX_HISTORY: usize = 200;

#[derive(Clone)]
struct Snapshot {
    doc: Document,
    anchor: Pos,
    head: Pos,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Typing,
    Deleting,
    Other,
}

/// Where a search starts from the current selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindDir {
    /// The first match starting at or after the selection (keeps the current match).
    Current,
    Next,
    Prev,
}

pub struct Editor {
    doc: Document,
    anchor: Pos,
    head: Pos,
    /// Format for the next typed text when the caret has no selection.
    pending: Option<Format>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last: Option<Action>,
    /// Where the caret was in each story when it was last left.
    saved: [Pos; 3],
    /// Bumped by every edit, undo and redo; lets a host tell whether the
    /// document changed since it was saved.
    revision: u64,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new(Document::default())
    }
}

impl Editor {
    pub fn new(doc: Document) -> Self {
        Self {
            doc,
            anchor: Pos::default(),
            head: Pos::default(),
            pending: None,
            undo: vec![],
            redo: vec![],
            last: None,
            saved: [Pos::default(); 3],
            revision: 0,
        }
    }

    /// Replace the document and clear history.
    pub fn load(&mut self, doc: Document) {
        *self = Editor::new(doc);
    }

    pub fn doc(&self) -> &Document {
        &self.doc
    }

    pub fn head(&self) -> Pos {
        self.head
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.head
    }

    /// Selection as an ordered `(start, end)` pair (equal when collapsed).
    pub fn selection(&self) -> (Pos, Pos) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    /// Selected text for the clipboard (images are left out).
    pub fn selected_text(&self) -> String {
        let (s, e) = self.selection();
        self.doc
            .text_in_range(s, e)
            .chars()
            .filter(|&c| c != OBJECT_CHAR && !is_field_char(c))
            .collect()
    }

    /// Changes whenever the document does (edits, undo, redo).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    // ---- cursor ----------------------------------------------------------

    pub fn set_cursor(&mut self, pos: Pos, extend: bool) {
        self.head = self.doc.clamp(pos);
        if !extend {
            self.anchor = self.head;
        }
        self.pending = None;
        self.last = None;
    }

    pub fn move_left(&mut self, extend: bool) {
        let to = if self.has_selection() && !extend {
            self.selection().0
        } else {
            self.doc.move_left(self.head)
        };
        self.set_cursor(to, extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        let to = if self.has_selection() && !extend {
            self.selection().1
        } else {
            self.doc.move_right(self.head)
        };
        self.set_cursor(to, extend);
    }

    pub fn select_all(&mut self) {
        self.anchor = Pos::default();
        self.head = self.doc.end();
        self.pending = None;
        self.last = None;
    }

    pub fn select_word(&mut self, pos: Pos) {
        let (s, e) = self.doc.word_range(pos);
        self.anchor = s;
        self.head = e;
        self.pending = None;
        self.last = None;
    }

    pub fn select_paragraph(&mut self, para: usize) {
        let para = para.min(self.doc.paragraphs.len() - 1);
        self.anchor = Pos { para, offset: 0 };
        self.head = Pos {
            para,
            offset: self.doc.paragraphs[para].len(),
        };
        self.pending = None;
        self.last = None;
    }

    // ---- history ---------------------------------------------------------

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            doc: self.doc.clone(),
            anchor: self.anchor,
            head: self.head,
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.doc = s.doc;
        self.anchor = s.anchor;
        self.head = s.head;
        self.pending = None;
        self.last = None;
    }

    /// Run a mutation, recording an undo step unless it coalesces with the
    /// previous edit or turns out to change nothing.
    fn edit(&mut self, action: Action, f: impl FnOnce(&mut Self)) {
        let coalesce = action != Action::Other && self.last == Some(action);
        let snap = (!coalesce).then(|| self.snapshot());
        f(self);
        match snap {
            Some(s) => {
                if s.doc != self.doc {
                    self.revision += 1;
                    self.undo.push(s);
                    if self.undo.len() > MAX_HISTORY {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                    self.last = Some(action);
                }
            }
            None => {
                self.revision += 1;
                self.redo.clear();
            }
        }
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            let cur = self.snapshot();
            self.redo.push(cur);
            self.restore(prev);
            self.revision += 1;
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            let cur = self.snapshot();
            self.undo.push(cur);
            self.restore(next);
            self.revision += 1;
        }
    }

    // ---- edits -----------------------------------------------------------

    fn delete_selection(&mut self) {
        if self.has_selection() {
            let (s, e) = self.selection();
            self.head = self.doc.delete_range(s, e);
            self.anchor = self.head;
        }
    }

    /// Type or paste text, replacing any selection.
    pub fn insert_text(&mut self, text: &str) {
        let text: String = text
            .chars()
            .filter(|&c| c != OBJECT_CHAR && !is_field_char(c))
            .collect();
        let text = text.as_str();
        if text.is_empty() {
            return;
        }
        let action = if text.contains('\n') {
            Action::Other
        } else {
            Action::Typing
        };
        self.edit(action, |e| {
            e.delete_selection();
            let fmt = e.pending.take();
            e.head = e.doc.insert_text(e.head, text, fmt.as_ref());
            e.anchor = e.head;
        });
    }

    pub fn enter(&mut self) {
        self.edit(Action::Other, |e| {
            e.delete_selection();
            let p = &mut e.doc.paragraphs[e.head.para];
            // Enter on an empty list item ends the list.
            if p.kind.is_list() && p.is_empty() {
                p.kind = Kind::Normal;
                return;
            }
            let was = (p.kind, e.head.offset == p.len());
            e.head = e.doc.split_paragraph(e.head);
            if let (Kind::Heading(_), true) = was {
                e.doc.paragraphs[e.head.para].kind = Kind::Normal;
            }
            e.anchor = e.head;
            e.pending = None;
        });
    }

    pub fn backspace(&mut self) {
        self.edit(Action::Deleting, |e| {
            if e.has_selection() {
                e.delete_selection();
            } else {
                e.head = e.doc.backspace(e.head);
                e.anchor = e.head;
            }
        });
    }

    pub fn delete_forward(&mut self) {
        self.edit(Action::Deleting, |e| {
            if e.has_selection() {
                e.delete_selection();
            } else {
                e.head = e.doc.delete_forward(e.head);
                e.anchor = e.head;
            }
        });
    }

    pub fn cut(&mut self) -> String {
        let text = self.selected_text();
        if !text.is_empty() {
            self.edit(Action::Other, |e| e.delete_selection());
        }
        text
    }

    /// Toggle an attribute on the selection, or for upcoming typing when
    /// nothing is selected.
    pub fn toggle_attr(&mut self, a: Attr) {
        if self.has_selection() {
            let (s, e) = self.selection();
            let on = !self.doc.range_all(s, e, a);
            self.edit(Action::Other, |ed| {
                ed.doc.set_format_range(s, e, |f| f.set(a, on))
            });
        } else {
            let mut f = self.current_format();
            f.set(a, !f.get(a));
            self.pending = Some(f);
        }
    }

    /// Set the kind on every selected paragraph; if they all already have it,
    /// revert them to `Normal`.
    pub fn toggle_kind(&mut self, kind: Kind) {
        let (s, e) = self.selection();
        let all = (s.para..=e.para).all(|i| self.doc.paragraphs[i].kind == kind);
        let to = if all { Kind::Normal } else { kind };
        self.edit(Action::Other, |ed| {
            for i in s.para..=e.para {
                ed.doc.paragraphs[i].kind = to;
            }
        });
    }

    // ---- character and paragraph formatting ------------------------------

    /// Apply a format change to the selection, or to upcoming typing when
    /// nothing is selected.
    fn apply_format(&mut self, f: impl Fn(&mut Format)) {
        if self.has_selection() {
            let (s, e) = self.selection();
            self.edit(Action::Other, |ed| ed.doc.set_format_range(s, e, f));
        } else {
            let mut fmt = self.current_format();
            f(&mut fmt);
            self.pending = Some(fmt);
        }
    }

    /// Font size in half-points; `None` returns to the paragraph default.
    pub fn set_size(&mut self, half_points: Option<u16>) {
        self.apply_format(|f| f.size = half_points.map(|v| v.clamp(2, 1000)));
    }

    pub fn set_font(&mut self, font: Option<&str>) {
        let font: Option<std::sync::Arc<str>> = font.filter(|n| !n.is_empty()).map(Into::into);
        self.apply_format(|f| f.font = font.clone());
    }

    /// Text colour as `0xRRGGBB`; `None` returns to the default.
    pub fn set_color(&mut self, color: Option<u32>) {
        self.apply_format(|f| f.color = color.map(|c| c & 0xFF_FFFF));
    }

    /// Align every selected paragraph.
    pub fn set_align(&mut self, align: Align) {
        let (s, e) = self.selection();
        self.edit(Action::Other, |ed| {
            for i in s.para..=e.para {
                ed.doc.paragraphs[i].align = align;
            }
        });
    }

    pub fn current_align(&self) -> Align {
        self.doc.paragraphs[self.head.para].align
    }

    /// Line spacing for the selected paragraphs in 240ths (240 single, 360
    /// one-and-a-half, 480 double); `None` restores the default.
    pub fn set_line_spacing(&mut self, line: Option<u16>) {
        let (s, e) = self.selection();
        self.edit(Action::Other, |ed| {
            for i in s.para..=e.para {
                ed.doc.paragraphs[i].line_spacing = line;
            }
        });
    }

    /// Indent the selected paragraphs by `delta` twips (negative outdents).
    pub fn indent(&mut self, delta: i32) {
        let (s, e) = self.selection();
        self.edit(Action::Other, |ed| {
            for i in s.para..=e.para {
                let p = &mut ed.doc.paragraphs[i];
                let v = (p.indent_left.unwrap_or(0) as i32 + delta).clamp(0, 12000);
                p.indent_left = (v > 0).then_some(v as u16);
            }
        });
    }

    pub fn set_page(&mut self, page: Page) {
        self.edit(Action::Other, |ed| ed.doc.page = page);
    }

    pub fn current_paragraph(&self) -> &Paragraph {
        &self.doc.paragraphs[self.head.para]
    }

    // ---- find and replace --------------------------------------------------

    /// Select a match of `query`, wrapping around the story. Returns whether
    /// anything matched.
    pub fn find(&mut self, query: &str, opts: FindOptions, dir: FindDir) -> bool {
        let matches = self.doc.find_all(query, opts);
        let (s, e) = self.selection();
        let pick = match dir {
            FindDir::Current => matches.iter().find(|m| m.0 >= s),
            FindDir::Next => matches.iter().find(|m| m.0 >= e),
            FindDir::Prev => matches.iter().rev().find(|m| m.1 <= s),
        };
        let wrapped = match dir {
            FindDir::Prev => matches.last(),
            _ => matches.first(),
        };
        let Some(&(from, to)) = pick.or(wrapped) else {
            return false;
        };
        self.anchor = from;
        self.head = to;
        self.pending = None;
        self.last = None;
        true
    }

    /// `(number of matches, index of the match that is currently selected)`.
    pub fn find_status(&self, query: &str, opts: FindOptions) -> (usize, Option<usize>) {
        let matches = self.doc.find_all(query, opts);
        let sel = self.selection();
        let index = matches.iter().position(|&m| m == sel);
        (matches.len(), index)
    }

    /// Replace `[s, e)` with `text`, keeping the formatting of its first char.
    fn replace_range(&mut self, s: Pos, e: Pos, text: &str) -> Pos {
        let fmt = self.doc.format_at(Pos {
            para: s.para,
            offset: s.offset + 1,
        });
        self.doc.delete_range(s, e);
        let clean: String = text
            .chars()
            .filter(|&c| c != OBJECT_CHAR && !is_field_char(c))
            .collect();
        self.doc.insert_text(s, &clean, Some(&fmt))
    }

    /// Replace the selected match, then select the next one. If the selection
    /// is not a match, just select the next match. Returns whether a match
    /// was replaced.
    pub fn replace_current(&mut self, query: &str, opts: FindOptions, with: &str) -> bool {
        let sel = self.selection();
        if !self.doc.find_all(query, opts).contains(&sel) {
            self.find(query, opts, FindDir::Current);
            return false;
        }
        self.edit(Action::Other, |e| {
            let end = e.replace_range(sel.0, sel.1, with);
            e.head = end;
            e.anchor = end;
            e.pending = None;
        });
        self.find(query, opts, FindDir::Next);
        true
    }

    /// Replace every match as one undoable step; returns how many.
    pub fn replace_all(&mut self, query: &str, opts: FindOptions, with: &str) -> usize {
        let matches = self.doc.find_all(query, opts);
        if matches.is_empty() {
            return 0;
        }
        self.edit(Action::Other, |e| {
            // Back to front, so earlier offsets stay valid.
            let mut last = matches[0].0;
            for &(s, end) in matches.iter().rev() {
                last = e.replace_range(s, end, with);
            }
            e.head = e.doc.clamp(last);
            e.anchor = e.head;
            e.pending = None;
        });
        matches.len()
    }

    // ---- headers and footers -----------------------------------------------

    /// The story being edited.
    pub fn story(&self) -> Story {
        self.doc.story
    }

    /// Switch to editing the body, header or footer, remembering the caret.
    pub fn enter_story(&mut self, story: Story) {
        if story == self.doc.story {
            return;
        }
        self.saved[self.doc.story as usize] = self.head;
        self.doc.switch_story(story);
        let to = self.doc.clamp(self.saved[story as usize]);
        self.head = to;
        self.anchor = to;
        self.pending = None;
        self.last = None;
    }

    /// Insert a page-number (`PAGE_CHAR`) or page-count (`PAGES_CHAR`) field.
    /// Only headers and footers can hold fields.
    pub fn insert_field(&mut self, field: char) {
        if self.doc.story == Story::Body || !is_field_char(field) {
            return;
        }
        self.edit(Action::Other, |e| {
            e.delete_selection();
            let fmt = e.pending.take();
            e.head = e.doc.insert_text(e.head, &field.to_string(), fmt.as_ref());
            e.anchor = e.head;
        });
    }

    /// Give the document a centred "page number" footer, replacing the footer.
    pub fn add_page_numbers(&mut self) {
        self.edit(Action::Other, |e| {
            let page = Paragraph {
                align: Align::Center,
                runs: vec![Run::new(PAGE_CHAR.to_string(), Format::default())],
                ..Default::default()
            };
            e.doc.set_story_paras(Story::Footer, vec![page]);
            if e.doc.story == Story::Footer {
                e.head = e.doc.clamp(e.head);
                e.anchor = e.head;
            }
        });
    }

    // ---- images ----------------------------------------------------------

    /// Insert an inline image (display size in px) at the caret.
    pub fn insert_image(&mut self, mime: &str, data: Vec<u8>, width: u32, height: u32) {
        if self.doc.story != Story::Body {
            return; // headers and footers hold text and fields only
        }
        self.edit(Action::Other, |e| {
            e.delete_selection();
            e.head = e
                .doc
                .insert_image(e.head, Image::new(mime, data, width, height));
            e.anchor = e.head;
            e.pending = None;
        });
    }

    pub fn image_selected(&self) -> bool {
        let (s, e) = self.selection();
        !self.doc.images_in_range(s, e).is_empty()
    }

    /// Scale every selected image by `factor`.
    pub fn scale_selected_images(&mut self, factor: f32) {
        let (s, e) = self.selection();
        let ids = self.doc.images_in_range(s, e);
        if ids.is_empty() {
            return;
        }
        self.edit(Action::Other, |ed| ed.doc.scale_images(&ids, factor));
    }

    // ---- tables ----------------------------------------------------------

    pub fn in_table(&self) -> bool {
        self.doc.cell_at(self.head.para).is_some()
    }

    /// Insert a `rows` × `cols` table at the caret (clamped to 1..=50 × 1..=20).
    /// Does nothing inside an existing table.
    pub fn insert_table(&mut self, rows: usize, cols: usize) {
        let (rows, cols) = (rows.clamp(1, 50), cols.clamp(1, 20));
        if self.doc.story != Story::Body {
            return;
        }
        self.edit(Action::Other, |e| {
            e.delete_selection();
            if let Some(p) = e.doc.insert_table(e.head, rows, cols) {
                e.head = p;
                e.anchor = p;
                e.pending = None;
            }
        });
    }

    /// Apply a structural table edit at the caret; `f` returns the new caret.
    fn table_edit(&mut self, f: impl FnOnce(&mut Document, usize) -> Option<Pos>) {
        if !self.in_table() {
            return;
        }
        self.edit(Action::Other, |e| {
            if let Some(p) = f(&mut e.doc, e.head.para) {
                e.head = p;
                e.anchor = p;
                e.pending = None;
            }
        });
    }

    pub fn add_row(&mut self) {
        self.table_edit(|d, p| d.add_row(p));
    }

    pub fn add_col(&mut self) {
        self.table_edit(|d, p| d.add_col(p));
    }

    pub fn delete_row(&mut self) {
        self.table_edit(|d, p| d.delete_row(p));
    }

    pub fn delete_col(&mut self) {
        self.table_edit(|d, p| d.delete_col(p));
    }

    pub fn delete_table(&mut self) {
        self.table_edit(|d, p| d.delete_table(p));
    }

    /// Tab: in a table, move to the next cell (adding a row after the last
    /// one); Shift+Tab moves back. Outside a table, Tab types a tab character.
    pub fn tab(&mut self, backwards: bool) {
        let Some(c) = self.doc.cell_at(self.head.para) else {
            if !backwards {
                self.insert_text("\t");
            }
            return;
        };
        let (rows, cols) = self.doc.table_dims(c.table).unwrap_or((1, 1));
        let (r, k) = (c.row as usize, c.col as usize);
        let target = if backwards {
            match (r, k) {
                (0, 0) => None,
                (_, 0) => Some((r - 1, cols - 1)),
                _ => Some((r, k - 1)),
            }
        } else if k + 1 < cols {
            Some((r, k + 1))
        } else if r + 1 < rows {
            Some((r + 1, 0))
        } else {
            self.add_row();
            if let Some(p) = self.doc.cell_start(c.table, c.row + 1, 0) {
                self.set_cursor(p, false);
            }
            return;
        };
        if let Some((r, k)) = target {
            if let Some(p) = self.doc.cell_start(c.table, r as u16, k as u16) {
                self.set_cursor(p, false);
            }
        }
    }

    // ---- state for the UI ------------------------------------------------

    /// Format at the caret (or the pending format), for toolbar state.
    pub fn current_format(&self) -> Format {
        if let Some(f) = &self.pending {
            return f.clone();
        }
        if self.has_selection() {
            let (s, _) = self.selection();
            // Format of the first selected char.
            return self.doc.format_at(Pos {
                para: s.para,
                offset: s.offset + 1,
            });
        }
        self.doc.format_at(self.head)
    }

    pub fn current_kind(&self) -> Kind {
        self.doc.paragraphs[self.head.para].kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str) -> Editor {
        let mut e = Editor::default();
        e.insert_text(text);
        e
    }

    #[test]
    fn typing_replaces_selection() {
        let mut e = ed("hello world");
        e.set_cursor(Pos { para: 0, offset: 0 }, false);
        e.set_cursor(Pos { para: 0, offset: 5 }, true);
        e.insert_text("bye");
        assert_eq!(e.doc().plain_text(), "bye world");
        assert!(!e.has_selection());
    }

    #[test]
    fn delete_range_across_paragraphs() {
        let mut e = ed("one\ntwo\nthree");
        e.set_cursor(Pos { para: 0, offset: 2 }, false);
        e.set_cursor(Pos { para: 2, offset: 2 }, true);
        e.backspace();
        assert_eq!(e.doc().plain_text(), "onree");
        assert_eq!(e.head(), Pos { para: 0, offset: 2 });
    }

    #[test]
    fn undo_coalesces_typing_and_redo_restores() {
        let mut e = Editor::default();
        for c in "abc".chars() {
            e.insert_text(&c.to_string());
        }
        e.set_cursor(e.doc().end(), false);
        e.insert_text("d");
        e.undo();
        assert_eq!(e.doc().plain_text(), "abc");
        e.undo();
        assert_eq!(e.doc().plain_text(), "");
        assert!(!e.can_undo());
        e.redo();
        e.redo();
        assert_eq!(e.doc().plain_text(), "abcd");
    }

    #[test]
    fn noop_edits_do_not_pollute_history() {
        let mut e = Editor::default();
        e.backspace();
        assert!(!e.can_undo());
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut e = ed("a");
        e.undo();
        assert!(e.can_redo());
        e.insert_text("b");
        assert!(!e.can_redo());
    }

    #[test]
    fn bold_selection_toggles() {
        let mut e = ed("abcdef");
        e.set_cursor(Pos { para: 0, offset: 1 }, false);
        e.set_cursor(Pos { para: 0, offset: 4 }, true);
        e.toggle_attr(Attr::Bold);
        assert!(e.doc().range_all(
            Pos { para: 0, offset: 1 },
            Pos { para: 0, offset: 4 },
            Attr::Bold
        ));
        assert!(!e.doc().range_all(
            Pos { para: 0, offset: 0 },
            Pos { para: 0, offset: 6 },
            Attr::Bold
        ));
        e.toggle_attr(Attr::Bold);
        assert_eq!(e.doc().paragraphs[0].runs.len(), 1);
        e.undo();
        assert!(e.doc().range_all(
            Pos { para: 0, offset: 1 },
            Pos { para: 0, offset: 4 },
            Attr::Bold
        ));
    }

    #[test]
    fn pending_format_applies_to_next_typing_only() {
        let mut e = ed("a");
        e.toggle_attr(Attr::Italic);
        e.insert_text("b");
        e.insert_text("c");
        let runs = &e.doc().paragraphs[0].runs;
        assert_eq!(runs.len(), 2);
        assert!(runs[1].format.italic && runs[1].text == "bc");
    }

    #[test]
    fn enter_on_empty_list_item_exits_list() {
        let mut e = ed("item");
        e.toggle_kind(Kind::Bullet);
        e.enter();
        assert_eq!(e.doc().paragraphs[1].kind, Kind::Bullet);
        e.enter();
        assert_eq!(e.doc().paragraphs.len(), 2);
        assert_eq!(e.doc().paragraphs[1].kind, Kind::Normal);
    }

    #[test]
    fn enter_after_heading_gives_normal() {
        let mut e = ed("Title");
        e.toggle_kind(Kind::Heading(1));
        e.enter();
        assert_eq!(e.doc().paragraphs[1].kind, Kind::Normal);
    }

    #[test]
    fn backspace_at_start_demotes_then_joins() {
        let mut e = ed("a\nb");
        e.toggle_kind(Kind::Bullet);
        assert_eq!(e.doc().paragraphs[1].kind, Kind::Bullet);
        e.set_cursor(Pos { para: 1, offset: 0 }, false);
        e.backspace();
        assert_eq!(e.doc().paragraphs.len(), 2);
        assert_eq!(e.doc().paragraphs[1].kind, Kind::Normal);
        e.backspace();
        assert_eq!(e.doc().plain_text(), "ab");
    }

    #[test]
    fn toggle_kind_twice_restores_normal() {
        let mut e = ed("x");
        e.toggle_kind(Kind::Numbered);
        assert_eq!(e.current_kind(), Kind::Numbered);
        e.toggle_kind(Kind::Numbered);
        assert_eq!(e.current_kind(), Kind::Normal);
    }

    #[test]
    fn select_word_and_cut() {
        let mut e = ed("foo bar baz");
        e.select_word(Pos { para: 0, offset: 5 });
        assert_eq!(e.selected_text(), "bar");
        assert_eq!(e.cut(), "bar");
        assert_eq!(e.doc().plain_text(), "foo  baz");
    }

    #[test]
    fn arrows_collapse_selection() {
        let mut e = ed("abcd");
        e.select_all();
        e.move_left(false);
        assert_eq!(e.head(), Pos::default());
        e.select_all();
        e.move_right(false);
        assert_eq!(e.head(), Pos { para: 0, offset: 4 });
    }

    #[test]
    fn text_in_range_spans_paragraphs() {
        let d = Document::from_paragraphs(vec![Paragraph::plain("ab"), Paragraph::plain("cd")]);
        assert_eq!(
            d.text_in_range(Pos { para: 0, offset: 1 }, Pos { para: 1, offset: 1 }),
            "b\nc"
        );
    }

    fn cells(e: &Editor) -> Vec<(u16, u16, String)> {
        e.doc()
            .paragraphs
            .iter()
            .filter_map(|p| p.cell.map(|c| (c.row, c.col, p.text())))
            .collect()
    }

    #[test]
    fn insert_table_splits_paragraph_and_leaves_trailing_paragraph() {
        let mut e = ed("ab");
        e.set_cursor(Pos { para: 0, offset: 1 }, false);
        e.insert_table(2, 3);
        let d = e.doc();
        assert_eq!(d.paragraphs.len(), 1 + 6 + 1);
        assert_eq!(d.paragraphs[0].text(), "a");
        assert_eq!(d.paragraphs[7].text(), "b");
        assert_eq!(e.head(), Pos { para: 1, offset: 0 });
        assert!(e.in_table());
        e.undo();
        assert_eq!(e.doc().plain_text(), "ab");
    }

    #[test]
    fn no_nested_tables() {
        let mut e = ed("x");
        e.insert_table(2, 2);
        let n = e.doc().paragraphs.len();
        e.insert_table(2, 2);
        assert_eq!(e.doc().paragraphs.len(), n);
    }

    #[test]
    fn tab_walks_cells_and_appends_a_row() {
        let mut e = ed("x");
        e.insert_table(1, 2);
        e.insert_text("A");
        e.tab(false);
        e.insert_text("B");
        e.tab(false); // last cell → new row
        e.insert_text("C");
        assert_eq!(
            cells(&e),
            vec![
                (0, 0, "A".into()),
                (0, 1, "B".into()),
                (1, 0, "C".into()),
                (1, 1, "".into())
            ]
        );
        e.tab(true);
        assert_eq!(
            e.doc().cell_at(e.head().para).map(|c| (c.row, c.col)),
            Some((0, 1))
        );
    }

    #[test]
    fn tab_outside_table_types_a_tab() {
        let mut e = ed("a");
        e.tab(false);
        assert_eq!(e.doc().plain_text(), "a\t");
        e.tab(true);
        assert_eq!(e.doc().plain_text(), "a\t");
    }

    #[test]
    fn backspace_never_crosses_cell_boundaries() {
        let mut e = ed("x");
        e.insert_table(1, 2);
        e.tab(false);
        let before = e.doc().clone();
        e.backspace(); // at the start of cell (0,1)
        assert_eq!(e.doc(), &before);
        e.set_cursor(Pos { para: 1, offset: 0 }, false);
        e.backspace(); // at the start of the first cell, after plain "x"? "x" is before the table
        assert_eq!(e.doc(), &before);
    }

    #[test]
    fn delete_forward_stops_at_cell_end() {
        let mut e = ed("x");
        e.insert_table(1, 2);
        e.insert_text("A");
        let before = e.doc().clone();
        e.delete_forward();
        assert_eq!(e.doc(), &before);
    }

    #[test]
    fn selection_delete_across_cells_keeps_structure() {
        let mut e = ed("top");
        e.insert_table(2, 2);
        for (i, t) in ["a", "b", "c", "d"].into_iter().enumerate() {
            e.insert_text(t);
            if i < 3 {
                e.tab(false);
            }
        }
        // select from inside cell (0,0) to inside cell (1,1) and delete
        let first = e.doc().cell_start(0, 0, 0).unwrap();
        let last = e.doc().cell_start(0, 1, 1).unwrap();
        e.set_cursor(first, false);
        e.set_cursor(
            Pos {
                para: last.para,
                offset: 1,
            },
            true,
        );
        e.backspace();
        let c = cells(&e);
        assert_eq!(c.len(), 4, "grid intact: {c:?}");
        assert!(c.iter().all(|(_, _, t)| t.is_empty()));
    }

    #[test]
    fn selection_spanning_text_and_table_deletes_text_only() {
        let mut e = ed("hello");
        e.insert_table(1, 1);
        e.insert_text("cell");
        e.set_cursor(Pos { para: 0, offset: 2 }, false);
        e.set_cursor(Pos { para: 1, offset: 2 }, true);
        e.backspace();
        assert_eq!(e.doc().paragraphs[0].text(), "he");
        assert_eq!(e.doc().paragraphs[1].text(), "ll");
        assert!(e.doc().paragraphs[1].cell.is_some());
    }

    #[test]
    fn add_and_delete_rows_and_cols() {
        let mut e = ed("x");
        e.insert_table(2, 2);
        let t = e.doc().cell_at(e.head().para).unwrap().table;
        e.add_col();
        assert_eq!(e.doc().table_dims(t), Some((2, 3)));
        assert_eq!(
            e.doc().cell_at(e.head().para).map(|c| (c.row, c.col)),
            Some((0, 1))
        );
        e.add_row();
        assert_eq!(e.doc().table_dims(t), Some((3, 3)));
        e.delete_row();
        e.delete_col();
        assert_eq!(e.doc().table_dims(t), Some((2, 2)));
        e.undo();
        assert_eq!(e.doc().table_dims(t), Some((2, 3)));
    }

    #[test]
    fn deleting_last_row_removes_table() {
        let mut e = ed("x");
        e.insert_table(1, 2);
        e.delete_row();
        assert!(e.doc().paragraphs.iter().all(|p| p.cell.is_none()));
        assert!(!e.in_table());
    }

    #[test]
    fn delete_table_removes_all_cells() {
        let mut e = ed("x");
        e.insert_table(3, 3);
        e.delete_table();
        assert_eq!(e.doc().plain_text(), "x\n");
    }

    #[test]
    fn enter_inside_cell_stays_in_cell() {
        let mut e = ed("x");
        e.insert_table(1, 1);
        e.insert_text("ab");
        e.enter();
        e.insert_text("cd");
        let c = cells(&e);
        assert_eq!(c, vec![(0, 0, "ab".into()), (0, 0, "cd".into())]);
    }

    #[test]
    fn repair_fills_ragged_and_adjacent_tables() {
        let cell = |t, r, c, s: &str| Paragraph {
            cell: Some(Cell {
                table: t,
                row: r,
                col: c,
            }),
            ..Paragraph::plain(s)
        };
        let d = Document::from_paragraphs(vec![
            cell(0, 0, 0, "a"),
            cell(0, 0, 1, "b"),
            cell(0, 1, 0, "c"),
            cell(1, 0, 0, "z"),
        ]);
        assert_eq!(d.table_dims(0), Some((2, 2)));
        assert_eq!(
            d.paragraphs.iter().filter(|p| p.cell.is_none()).count(),
            2,
            "separator between tables and a trailing paragraph"
        );
        assert!(d.paragraphs.last().unwrap().cell.is_none());
    }

    fn png() -> Vec<u8> {
        vec![0x89, b'P', b'N', b'G']
    }

    #[test]
    fn insert_image_is_one_char_and_typing_after_it_is_text() {
        let mut e = ed("ab");
        e.set_cursor(Pos { para: 0, offset: 1 }, false);
        e.insert_image("image/png", png(), 100, 50);
        assert_eq!(e.doc().paragraphs[0].len(), 3);
        assert_eq!(e.head(), Pos { para: 0, offset: 2 });
        e.insert_text("X");
        let runs = &e.doc().paragraphs[0].runs;
        assert_eq!(runs.len(), 3, "a | image | Xb: {runs:?}");
        assert_eq!(runs[1].format.image, Some(0));
        assert_eq!(runs[2].format.image, None);
        assert_eq!(runs[2].text, "Xb");
        e.undo();
        e.undo();
        assert_eq!(e.doc().plain_text(), "ab");
    }

    #[test]
    fn image_chars_never_leak_into_typed_or_copied_text() {
        let mut e = ed("a");
        e.insert_image("image/png", png(), 10, 10);
        e.insert_text(&format!("x{OBJECT_CHAR}y"));
        assert_eq!(e.doc().paragraphs[0].len(), 4);
        e.select_all();
        assert_eq!(e.selected_text(), "axy");
    }

    #[test]
    fn select_scale_and_delete_image() {
        let mut e = ed("");
        e.insert_image("image/png", png(), 100, 40);
        e.set_cursor(Pos { para: 0, offset: 0 }, false);
        assert!(!e.image_selected());
        e.select_word(Pos { para: 0, offset: 0 });
        assert!(e.image_selected(), "double-click selects exactly the image");
        e.scale_selected_images(2.0);
        assert_eq!(
            (e.doc().images[0].width, e.doc().images[0].height),
            (200, 80)
        );
        e.scale_selected_images(0.001);
        assert_eq!(e.doc().images[0].width, 16, "clamped");
        e.undo();
        assert_eq!(e.doc().images[0].width, 200);
        e.backspace();
        assert!(e.doc().paragraphs[0].is_empty());
    }

    #[test]
    fn undo_does_not_clone_image_bytes() {
        let mut e = ed("");
        e.insert_image("image/png", vec![7; 1 << 20], 10, 10);
        e.insert_text("a");
        e.insert_text("b");
        let first = e.doc().images[0].data.clone();
        e.undo();
        e.redo();
        assert!(std::sync::Arc::ptr_eq(&first, &e.doc().images[0].data));
    }

    #[test]
    fn size_font_color_apply_to_selection_and_typing() {
        let mut e = ed("abcd");
        e.set_cursor(Pos { para: 0, offset: 1 }, false);
        e.set_cursor(Pos { para: 0, offset: 3 }, true);
        e.set_size(Some(28));
        e.set_font(Some("Arial"));
        e.set_color(Some(0x112233));
        let runs = &e.doc().paragraphs[0].runs;
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].format.size, Some(28));
        assert_eq!(runs[1].format.font.as_deref(), Some("Arial"));
        assert_eq!(runs[1].format.color, Some(0x112233));
        assert_eq!(runs[0].format.size, None);
        // Caret only: applies to what is typed next, then sticks to that text.
        e.set_cursor(Pos { para: 0, offset: 4 }, false);
        e.set_size(Some(40));
        e.insert_text("Z");
        e.insert_text("Y");
        let last = e.doc().paragraphs[0].runs.last().unwrap();
        assert_eq!((last.text.as_str(), last.format.size), ("ZY", Some(40)));
        // Clearing returns to defaults.
        e.select_all();
        e.set_size(None);
        e.set_font(None);
        e.set_color(None);
        assert_eq!(e.doc().paragraphs[0].runs.len(), 1);
        for _ in 0..3 {
            e.undo(); // one step per property
        }
        assert_eq!(e.doc().paragraphs[0].runs.len(), 4);
    }

    #[test]
    fn typing_inherits_size_from_previous_text() {
        let mut e = ed("ab");
        e.select_all();
        e.set_size(Some(30));
        e.set_cursor(Pos { para: 0, offset: 2 }, false);
        e.insert_text("c");
        assert_eq!(e.doc().paragraphs[0].runs.len(), 1);
        assert_eq!(e.doc().paragraphs[0].runs[0].format.size, Some(30));
    }

    #[test]
    fn alignment_applies_to_selected_paragraphs_and_survives_enter() {
        let mut e = ed("a\nb\nc");
        e.set_cursor(Pos { para: 0, offset: 0 }, false);
        e.set_cursor(Pos { para: 1, offset: 1 }, true);
        e.set_align(Align::Center);
        let a: Vec<_> = e.doc().paragraphs.iter().map(|p| p.align).collect();
        assert_eq!(a, [Align::Center, Align::Center, Align::Left]);
        e.set_cursor(Pos { para: 1, offset: 1 }, false);
        e.enter();
        assert_eq!(e.doc().paragraphs[2].align, Align::Center);
        assert_eq!(e.current_align(), Align::Center);
        e.undo();
        assert_eq!(e.doc().paragraphs.len(), 3);
    }

    #[test]
    fn spacing_indent_and_page_edits() {
        let mut e = ed("a\nb");
        e.select_all();
        e.set_line_spacing(Some(360));
        e.indent(720);
        e.indent(720);
        assert!(e
            .doc()
            .paragraphs
            .iter()
            .all(|p| p.line_spacing == Some(360) && p.indent_left == Some(1440)));
        e.indent(-5000);
        assert_eq!(
            e.doc().paragraphs[0].indent_left,
            None,
            "outdent bottoms out at zero"
        );
        e.set_cursor(Pos { para: 0, offset: 1 }, false);
        e.set_line_spacing(Some(480));
        e.enter();
        assert_eq!(
            e.doc().paragraphs[1].line_spacing,
            Some(480),
            "Enter keeps paragraph properties"
        );
        e.set_page(Page::A4);
        assert_eq!(e.doc().page, Page::A4);
        e.undo();
        assert_eq!(e.doc().page, Page::LETTER);
        assert_eq!(Page::px(1440), 96.0);
    }

    #[test]
    fn stories_keep_their_own_content_and_carets() {
        let mut e = ed("body text");
        e.enter_story(Story::Header);
        assert_eq!(e.story(), Story::Header);
        assert_eq!(
            e.doc().plain_text(),
            "body text",
            "plain_text is always the body"
        );
        e.insert_text("Top");
        e.enter_story(Story::Footer);
        e.insert_text("Bottom");
        e.enter_story(Story::Body);
        assert_eq!(e.head(), Pos { para: 0, offset: 9 }, "body caret restored");
        e.insert_text("!");
        assert_eq!(e.doc().plain_text(), "body text!");
        assert_eq!(e.doc().story_paras(Story::Header)[0].text(), "Top");
        assert_eq!(e.doc().story_paras(Story::Footer)[0].text(), "Bottom");
        assert!(e.doc().story_has_content(Story::Header));
        e.enter_story(Story::Header);
        assert_eq!(e.head(), Pos { para: 0, offset: 3 });
    }

    #[test]
    fn undo_follows_edits_across_stories() {
        let mut e = ed("a");
        e.enter_story(Story::Header);
        e.insert_text("H");
        e.enter_story(Story::Body);
        e.insert_text("b");
        e.undo(); // the body edit
        assert_eq!(e.doc().plain_text(), "a");
        e.undo(); // header edit: the editor returns to the header
        assert_eq!(e.story(), Story::Header);
        assert_eq!(e.doc().paragraphs[0].text(), "");
        e.redo(); // back to the state before that undo: body active, header "H" parked
        assert_eq!(e.story(), Story::Body);
        assert_eq!(e.doc().story_paras(Story::Header)[0].text(), "H");
    }

    #[test]
    fn fields_only_in_header_and_footer_and_never_from_typing() {
        let mut e = ed("x");
        e.insert_field(PAGE_CHAR);
        assert_eq!(e.doc().plain_text(), "x", "no fields in the body");
        e.enter_story(Story::Footer);
        e.insert_text("Page ");
        e.insert_field(PAGE_CHAR);
        e.insert_text(" of ");
        e.insert_field(PAGES_CHAR);
        e.insert_text(&format!("{PAGE_CHAR}{OBJECT_CHAR}z")); // pasted placeholders are dropped
        let p = &e.doc().paragraphs[0];
        assert_eq!(p.text(), format!("Page {PAGE_CHAR} of {PAGES_CHAR}z"));
        e.select_all();
        assert_eq!(
            e.selected_text(),
            "Page  of z",
            "fields are not copied as text"
        );
        let len = e.doc().paragraphs[0].len();
        e.insert_field('x'); // not a field character: ignored (selection stays)
        assert_eq!(e.doc().paragraphs[0].len(), len);
        assert!(e.has_selection());
    }

    #[test]
    fn page_numbers_shortcut_and_header_footer_restrictions() {
        let mut e = ed("body");
        e.add_page_numbers();
        let footer = &e.doc().story_paras(Story::Footer)[0];
        assert_eq!(
            (footer.align, footer.text()),
            (Align::Center, PAGE_CHAR.to_string())
        );
        assert_eq!(e.story(), Story::Body);
        e.undo();
        assert!(!e.doc().story_has_content(Story::Footer));
        e.redo();
        // Restrictions: no tables or images in headers/footers.
        e.enter_story(Story::Footer);
        let before = e.doc().clone();
        e.insert_table(2, 2);
        e.insert_image("image/png", vec![1], 10, 10);
        assert_eq!(e.doc(), &before);
        // Replacing the active footer keeps the caret valid.
        e.insert_text("zzz");
        e.add_page_numbers();
        assert_eq!(e.doc().paragraphs[0].text(), PAGE_CHAR.to_string());
        assert!(e.head().offset <= 1);
    }

    fn opts(case: bool, word: bool) -> FindOptions {
        FindOptions {
            match_case: case,
            whole_word: word,
        }
    }

    fn at(para: usize, offset: usize) -> Pos {
        Pos { para, offset }
    }

    #[test]
    fn find_all_handles_case_words_overlap_and_unicode() {
        let d = Document::from_paragraphs(vec![
            Paragraph::plain("Cat concat CAT cat_s, cat."),
            Paragraph::plain("ÉCOLE école aaaa"),
        ]);
        let n = |q: &str, o| d.find_all(q, o).len();
        assert_eq!(n("cat", opts(false, false)), 5);
        assert_eq!(n("cat", opts(true, false)), 3, "concat, cat_s, cat.");
        assert_eq!(
            n("cat", opts(false, true)),
            3,
            "Cat, CAT, cat. — not concat or cat_s"
        );
        assert_eq!(
            n("école", opts(false, false)),
            2,
            "case folding is Unicode-aware"
        );
        assert_eq!(n("école", opts(true, false)), 1);
        assert_eq!(n("aa", opts(true, false)), 2, "matches do not overlap");
        assert_eq!(n("", opts(false, false)), 0);
        assert_eq!(n("zzz", opts(false, false)), 0);
        assert_eq!(
            n("cat.", opts(true, true)),
            1,
            "punctuation edges skip the word-boundary check"
        );
        let m = d.find_all("école", opts(false, false));
        assert_eq!(m[0], (at(1, 0), at(1, 5)));
    }

    #[test]
    fn find_walks_forward_and_backward_with_wraparound() {
        let mut e = ed("x a x\nb x");
        let o = opts(true, false);
        e.set_cursor(Pos::default(), false);
        assert!(e.find("x", o, FindDir::Current));
        assert_eq!(
            e.selection(),
            (at(0, 0), at(0, 1)),
            "a match under the caret is kept"
        );
        assert!(e.find("x", o, FindDir::Next));
        assert_eq!(e.selection(), (at(0, 4), at(0, 5)));
        assert!(e.find("x", o, FindDir::Next));
        assert_eq!(e.selection(), (at(1, 2), at(1, 3)));
        assert!(e.find("x", o, FindDir::Next));
        assert_eq!(e.selection().0, at(0, 0), "wraps to the start");
        assert!(e.find("x", o, FindDir::Prev));
        assert_eq!(e.selection().0, at(1, 2), "and back to the end");
        assert!(e.find("x", o, FindDir::Prev));
        assert_eq!(e.selection().0, at(0, 4));
        assert!(!e.find("nope", o, FindDir::Next));
        assert_eq!(e.find_status("x", o), (3, Some(1)));
        e.set_cursor(at(0, 2), false);
        assert_eq!(e.find_status("x", o), (3, None));
    }

    #[test]
    fn replace_current_steps_through_matches_and_undoes() {
        let mut e = ed("one two one two one");
        let o = opts(true, false);
        e.set_cursor(Pos::default(), false);
        assert!(
            !e.replace_current("one", o, "1"),
            "first press only selects"
        );
        assert_eq!(e.selected_text(), "one");
        assert!(e.replace_current("one", o, "1"));
        assert_eq!(e.doc().plain_text(), "1 two one two one");
        assert_eq!(e.selected_text(), "one", "moved on to the next match");
        assert!(e.replace_current("one", o, "1"));
        assert!(e.replace_current("one", o, "1"));
        assert_eq!(e.doc().plain_text(), "1 two 1 two 1");
        assert!(!e.has_selection() || e.selected_text() != "one");
        e.undo();
        assert_eq!(e.doc().plain_text(), "1 two 1 two one");
    }

    #[test]
    fn replacement_containing_the_query_does_not_loop() {
        let mut e = ed("a a");
        let o = opts(true, false);
        e.set_cursor(Pos::default(), false);
        e.find("a", o, FindDir::Current);
        assert!(e.replace_current("a", o, "aa"));
        assert_eq!(e.doc().plain_text(), "aa a");
        assert_eq!(
            e.selection(),
            (at(0, 3), at(0, 4)),
            "skipped over the text just inserted"
        );
        assert_eq!(e.replace_all("a", o, "aa"), 3);
        assert_eq!(e.doc().plain_text(), "aaaa aa");
    }

    #[test]
    fn replace_all_is_one_undo_step_and_keeps_formatting() {
        let mut e = ed("foo bar foo\nfoo");
        e.set_cursor(at(0, 0), false);
        e.set_cursor(at(0, 3), true);
        e.toggle_attr(Attr::Bold);
        e.set_cursor(at(1, 0), false);
        let o = opts(true, false);
        assert_eq!(e.replace_all("foo", o, "quux"), 3);
        assert_eq!(e.doc().plain_text(), "quux bar quux\nquux");
        let runs = &e.doc().paragraphs[0].runs;
        assert!(
            runs[0].format.bold && runs[0].text == "quux",
            "bold first match stays bold: {runs:?}"
        );
        assert!(!runs.last().unwrap().format.bold);
        e.undo();
        assert_eq!(
            e.doc().plain_text(),
            "foo bar foo\nfoo",
            "one undo restores everything"
        );
        assert_eq!(e.replace_all("zzz", o, "y"), 0);
        assert!(e.can_undo());
    }

    #[test]
    fn replace_can_delete_split_and_never_insert_placeholders() {
        let mut e = ed("a-b-c");
        let o = opts(true, false);
        assert_eq!(e.replace_all("-", o, "\n"), 2);
        assert_eq!(e.doc().plain_text(), "a\nb\nc");
        assert_eq!(
            e.replace_all("\n", o, ""),
            0,
            "matches never span paragraphs"
        );
        let mut e = ed("a-b");
        assert_eq!(e.replace_all("-", o, ""), 1);
        assert_eq!(e.doc().plain_text(), "ab");
        let mut e = ed("a-b");
        e.replace_all("-", o, &format!("{OBJECT_CHAR}{PAGE_CHAR}x"));
        assert_eq!(e.doc().plain_text(), "axb");
    }

    #[test]
    fn find_works_in_table_cells_and_the_active_story() {
        let mut e = ed("top");
        e.insert_table(1, 2);
        e.insert_text("top cell");
        let o = opts(false, true);
        assert_eq!(e.doc().find_all("TOP", o).len(), 2);
        assert_eq!(e.replace_all("top", o, "head"), 2);
        assert!(e
            .doc()
            .paragraphs
            .iter()
            .any(|p| p.cell.is_some() && p.text() == "head cell"));
        // Searches look at the story being edited.
        e.enter_story(Story::Footer);
        assert_eq!(e.doc().find_all("head", o).len(), 0);
        e.insert_text("head");
        assert_eq!(e.doc().find_all("head", o).len(), 1);
    }

    #[test]
    fn revision_tracks_changes_but_not_cursor_moves_or_no_ops() {
        let mut e = Editor::default();
        let r0 = e.revision();
        e.set_cursor(Pos::default(), false);
        e.select_all();
        e.backspace(); // nothing to delete
        assert_eq!(e.revision(), r0, "moves and no-op edits don't count");
        e.insert_text("a");
        let r1 = e.revision();
        assert!(r1 > r0);
        e.insert_text("b"); // coalesced typing still counts
        assert!(e.revision() > r1);
        let r2 = e.revision();
        e.undo();
        assert!(e.revision() > r2);
        let r3 = e.revision();
        e.redo();
        assert!(e.revision() > r3);
        e.undo();
        e.undo(); // nothing left to undo
        let r4 = e.revision();
        e.undo();
        assert_eq!(e.revision(), r4);
        e.load(Document::default());
        assert_eq!(e.revision(), 0, "loading starts afresh");
    }
}
