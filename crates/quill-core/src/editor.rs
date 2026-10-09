//! Editing session: selection, pending format, and undo/redo history.
//!
//! History is snapshot-based with coalescing of consecutive typing/deleting.
//! (A merge-friendly operation log arrives with collaboration.)

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

pub struct Editor {
    doc: Document,
    anchor: Pos,
    head: Pos,
    /// Format for the next typed text when the caret has no selection.
    pending: Option<Format>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last: Option<Action>,
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

    pub fn selected_text(&self) -> String {
        let (s, e) = self.selection();
        self.doc.text_in_range(s, e)
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
                    self.undo.push(s);
                    if self.undo.len() > MAX_HISTORY {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                    self.last = Some(action);
                }
            }
            None => self.redo.clear(),
        }
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            let cur = self.snapshot();
            self.redo.push(cur);
            self.restore(prev);
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            let cur = self.snapshot();
            self.undo.push(cur);
            self.restore(next);
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

    // ---- tables ----------------------------------------------------------

    pub fn in_table(&self) -> bool {
        self.doc.cell_at(self.head.para).is_some()
    }

    /// Insert a `rows` × `cols` table at the caret (clamped to 1..=50 × 1..=20).
    /// Does nothing inside an existing table.
    pub fn insert_table(&mut self, rows: usize, cols: usize) {
        let (rows, cols) = (rows.clamp(1, 50), cols.clamp(1, 20));
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
}
