//! WebAssembly bindings for the spreadsheet engine.
//!
//! The page asks for the visible rectangle (`visible`) and gets back just
//! the cells it must draw, already formatted.

use quill_calc::{
    key_to_a1, parse_delimited, Align, Axis, CellRef, Clip, Key, NumFmt, Sheet, Value,
    DEFAULT_COL_WIDTH, DEFAULT_ROW_HEIGHT, MAX_COLS, MAX_ROWS,
};
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// One cell as the grid draws it.
#[derive(Serialize)]
struct CellView {
    r: u32,
    c: u32,
    text: String,
    /// "number", "text", "bool", "error" or "empty".
    kind: &'static str,
    bold: bool,
    italic: bool,
    /// Resolved alignment: "left", "center" or "right".
    align: &'static str,
    color: Option<u32>,
    fill: Option<u32>,
}

/// A cell's own formatting (for the toolbar's pressed states).
#[derive(Serialize)]
struct FormatView {
    bold: bool,
    italic: bool,
    /// "auto", "left", "center" or "right".
    align: &'static str,
    /// "general", "number", "integer", "percent" or "currency" (the closest match).
    numfmt: &'static str,
    color: Option<u32>,
    fill: Option<u32>,
}

#[derive(Serialize)]
struct Sizes {
    default_col: f32,
    default_row: f32,
    /// Columns/rows whose size was changed: `[index, size]` pairs.
    cols: Vec<(u32, f32)>,
    rows: Vec<(u32, f32)>,
}

#[derive(Serialize)]
struct Stats {
    count: usize,
    numbers: usize,
    sum: f64,
    min: Option<f64>,
    max: Option<f64>,
}

#[wasm_bindgen]
pub struct SheetEditor {
    sheet: Sheet,
    clip: Option<Clip>,
}

fn rect(r1: u32, c1: u32, r2: u32, c2: u32) -> (Key, Key) {
    ((r1.min(r2), c1.min(c2)), (r1.max(r2), c1.max(c2)))
}

fn parse_color(s: &str) -> Option<u32> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    (hex.len() == 6)
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
}

fn js_err(msg: impl Into<String>) -> JsError {
    JsError::new(&msg.into())
}

#[wasm_bindgen]
impl SheetEditor {
    #[wasm_bindgen(constructor)]
    pub fn new() -> SheetEditor {
        SheetEditor {
            sheet: Sheet::new(),
            clip: None,
        }
    }

    // ---- reading ---------------------------------------------------------

    /// Cells to draw inside the rectangle, with formatting resolved.
    pub fn visible(&self, r1: u32, r2: u32, c1: u32, c2: u32) -> Result<JsValue, JsError> {
        let (a, b) = rect(r1, c1, r2, c2);
        let views: Vec<CellView> = self
            .sheet
            .cells_in(a, b)
            .into_iter()
            .map(|(key, cell)| {
                let value = self.sheet.value(key);
                let st = &cell.data.style;
                let (kind, auto) = match value {
                    Value::Empty => ("empty", "left"),
                    Value::Number(_) => ("number", "right"),
                    Value::Text(_) => ("text", "left"),
                    Value::Bool(_) => ("bool", "center"),
                    Value::Error(_) => ("error", "center"),
                };
                CellView {
                    r: key.0,
                    c: key.1,
                    text: self.sheet.display(key),
                    kind,
                    bold: st.bold,
                    italic: st.italic,
                    align: match st.align {
                        Align::Auto => auto,
                        Align::Left => "left",
                        Align::Center => "center",
                        Align::Right => "right",
                    },
                    color: st.color,
                    fill: st.fill,
                }
            })
            .collect();
        serde_wasm_bindgen::to_value(&views).map_err(|e| js_err(e.to_string()))
    }

    /// Formatting of one cell.
    pub fn cell_format(&self, row: u32, col: u32) -> Result<JsValue, JsError> {
        let st = self.sheet.style((row, col));
        let view = FormatView {
            bold: st.bold,
            italic: st.italic,
            align: match st.align {
                Align::Auto => "auto",
                Align::Left => "left",
                Align::Center => "center",
                Align::Right => "right",
            },
            numfmt: match st.fmt {
                NumFmt::General => "general",
                NumFmt::Fixed { decimals: 0, .. } => "integer",
                NumFmt::Fixed { .. } => "number",
                NumFmt::Percent { .. } => "percent",
                NumFmt::Currency { .. } => "currency",
            },
            color: st.color,
            fill: st.fill,
        };
        serde_wasm_bindgen::to_value(&view).map_err(|e| js_err(e.to_string()))
    }

    /// What was typed into a cell (formulas start with `=`).
    pub fn raw(&self, row: u32, col: u32) -> String {
        self.sheet.raw((row, col))
    }

    pub fn display(&self, row: u32, col: u32) -> String {
        self.sheet.display((row, col))
    }

    /// `[rows, cols]` needed to show every non-empty cell.
    pub fn used_range(&self) -> Vec<u32> {
        let (r, c) = self.sheet.used_range();
        vec![r, c]
    }

    /// Sizes that differ from the default, as `{ default_col, default_row, cols, rows }`.
    pub fn sizes(&self) -> Result<JsValue, JsError> {
        let (rows, cols) = self.sheet.used_range();
        // Sizes can be set beyond the used range, so look further than the data.
        let probe_rows = rows.max(1000);
        let probe_cols = cols.max(60);
        let sizes = Sizes {
            default_col: DEFAULT_COL_WIDTH,
            default_row: DEFAULT_ROW_HEIGHT,
            cols: (0..probe_cols)
                .map(|c| (c, self.sheet.col_width(c)))
                .filter(|(_, w)| *w != DEFAULT_COL_WIDTH)
                .collect(),
            rows: (0..probe_rows)
                .map(|r| (r, self.sheet.row_height(r)))
                .filter(|(_, h)| *h != DEFAULT_ROW_HEIGHT)
                .collect(),
        };
        serde_wasm_bindgen::to_value(&sizes).map_err(|e| js_err(e.to_string()))
    }

    /// Count, sum and extremes of the numbers in a rectangle (for the status bar).
    pub fn stats(&self, r1: u32, c1: u32, r2: u32, c2: u32) -> Result<JsValue, JsError> {
        let (a, b) = rect(r1, c1, r2, c2);
        let mut st = Stats {
            count: 0,
            numbers: 0,
            sum: 0.0,
            min: None,
            max: None,
        };
        for (key, _) in self.sheet.cells_in(a, b) {
            match self.sheet.value(key) {
                Value::Empty => {}
                Value::Number(n) => {
                    st.count += 1;
                    st.numbers += 1;
                    st.sum += n;
                    st.min = Some(st.min.map_or(n, |m| m.min(n)));
                    st.max = Some(st.max.map_or(n, |m| m.max(n)));
                }
                _ => st.count += 1,
            }
        }
        serde_wasm_bindgen::to_value(&st).map_err(|e| js_err(e.to_string()))
    }

    /// Where Ctrl+Arrow goes from a cell: the edge of the data block in the
    /// direction `(dr, dc)` (each -1, 0 or 1). Returns `[row, col]`.
    pub fn edge(&self, row: u32, col: u32, dr: i32, dc: i32) -> Vec<u32> {
        let (rows, cols) = self.sheet.used_range();
        let filled = |r: i64, c: i64| {
            r >= 0 && c >= 0 && self.sheet.value((r as u32, c as u32)) != Value::Empty
        };
        let (mut r, mut c) = (row as i64, col as i64);
        let (max_r, max_c) = (MAX_ROWS as i64 - 1, MAX_COLS as i64 - 1);
        let inside = |r: i64, c: i64| (0..=max_r).contains(&r) && (0..=max_c).contains(&c);
        let (dr, dc) = (dr as i64, dc as i64);
        let beyond_data = |r: i64, c: i64| r > rows as i64 && dr > 0 || c > cols as i64 && dc > 0;
        if !inside(r + dr, c + dc) {
            return vec![r as u32, c as u32];
        }
        let next_filled = filled(r + dr, c + dc);
        if filled(r, c) && next_filled {
            // Inside a block: run to its last cell.
            while inside(r + dr, c + dc) && filled(r + dr, c + dc) {
                r += dr;
                c += dc;
            }
        } else {
            // Otherwise run to the next filled cell, or the sheet's edge.
            loop {
                if !inside(r + dr, c + dc) {
                    break;
                }
                r += dr;
                c += dc;
                if filled(r, c) {
                    break;
                }
                if beyond_data(r, c) || (dr < 0 && r == 0) || (dc < 0 && c == 0) {
                    // Nothing further in this direction: jump to the sheet's edge.
                    if dr > 0 {
                        r = max_r;
                    }
                    if dc > 0 {
                        c = max_c;
                    }
                    if dr < 0 {
                        r = 0;
                    }
                    if dc < 0 {
                        c = 0;
                    }
                    break;
                }
            }
        }
        vec![r as u32, c as u32]
    }

    // ---- editing -----------------------------------------------------------

    /// Type into a cell.
    pub fn set_input(&mut self, row: u32, col: u32, text: &str) {
        self.sheet.set_input((row, col), text);
    }

    pub fn clear_contents(&mut self, r1: u32, c1: u32, r2: u32, c2: u32) {
        let (a, b) = rect(r1, c1, r2, c2);
        self.sheet.clear_contents(a, b);
    }

    pub fn clear_all(&mut self, r1: u32, c1: u32, r2: u32, c2: u32) {
        let (a, b) = rect(r1, c1, r2, c2);
        self.sheet.clear_all(a, b);
    }

    /// Change one formatting property over a range. `prop` is one of
    /// `bold`/`italic` (`"1"`/`"0"`), `align` (`auto|left|center|right`),
    /// `color`/`fill` (`#rrggbb`, or `""` to clear) and `numfmt`
    /// (`general|number|integer|percent|currency`).
    pub fn style(
        &mut self,
        r1: u32,
        c1: u32,
        r2: u32,
        c2: u32,
        prop: &str,
        value: &str,
    ) -> Result<(), JsError> {
        let (a, b) = rect(r1, c1, r2, c2);
        match prop {
            "bold" => self.sheet.style_range(a, b, |s| s.bold = value == "1"),
            "italic" => self.sheet.style_range(a, b, |s| s.italic = value == "1"),
            "align" => {
                let align = match value {
                    "auto" => Align::Auto,
                    "left" => Align::Left,
                    "center" => Align::Center,
                    "right" => Align::Right,
                    other => return Err(js_err(format!("unknown alignment: {other}"))),
                };
                self.sheet.style_range(a, b, |s| s.align = align);
            }
            "color" => {
                let c = if value.is_empty() {
                    None
                } else {
                    Some(parse_color(value).ok_or_else(|| js_err("bad colour"))?)
                };
                self.sheet.style_range(a, b, |s| s.color = c);
            }
            "fill" => {
                let c = if value.is_empty() {
                    None
                } else {
                    Some(parse_color(value).ok_or_else(|| js_err("bad colour"))?)
                };
                self.sheet.style_range(a, b, |s| s.fill = c);
            }
            "numfmt" => {
                let fmt = match value {
                    "general" => NumFmt::General,
                    "number" => NumFmt::Fixed {
                        decimals: 2,
                        thousands: true,
                    },
                    "integer" => NumFmt::Fixed {
                        decimals: 0,
                        thousands: true,
                    },
                    "percent" => NumFmt::Percent { decimals: 0 },
                    "currency" => NumFmt::Currency { decimals: 2 },
                    other => return Err(js_err(format!("unknown number format: {other}"))),
                };
                self.sheet.style_range(a, b, |s| s.fmt = fmt);
            }
            other => return Err(js_err(format!("unknown style property: {other}"))),
        }
        Ok(())
    }

    /// Show one more (`delta` 1) or one fewer (`delta` -1) decimal place, based
    /// on the format of the range's first cell.
    pub fn adjust_decimals(&mut self, r1: u32, c1: u32, r2: u32, c2: u32, delta: i32) {
        let (a, b) = rect(r1, c1, r2, c2);
        let bump = |d: u8| (d as i32 + delta).clamp(0, 10) as u8;
        let current = self.sheet.style(a).fmt;
        let next = match current {
            NumFmt::General => NumFmt::Fixed {
                decimals: bump(0).max(if delta > 0 { 1 } else { 0 }),
                thousands: false,
            },
            NumFmt::Fixed {
                decimals,
                thousands,
            } => NumFmt::Fixed {
                decimals: bump(decimals),
                thousands,
            },
            NumFmt::Percent { decimals } => NumFmt::Percent {
                decimals: bump(decimals),
            },
            NumFmt::Currency { decimals } => NumFmt::Currency {
                decimals: bump(decimals),
            },
        };
        self.sheet.style_range(a, b, |s| s.fmt = next);
    }

    pub fn set_col_width(&mut self, col: u32, width: f32) {
        self.sheet.set_col_width(col, width);
    }

    pub fn set_row_height(&mut self, row: u32, height: f32) {
        self.sheet.set_row_height(row, height);
    }

    // ---- clipboard and fill ------------------------------------------------

    /// Remember a range (with formulas and formatting) for `paste`.
    pub fn copy(&mut self, r1: u32, c1: u32, r2: u32, c2: u32) {
        let (a, b) = rect(r1, c1, r2, c2);
        self.clip = Some(self.sheet.copy_range(a, b));
    }

    pub fn has_clip(&self) -> bool {
        self.clip.is_some()
    }

    /// Paste the remembered range with its top-left at `(row, col)`;
    /// returns the pasted area as `[r1, c1, r2, c2]`.
    pub fn paste(&mut self, row: u32, col: u32) -> Vec<u32> {
        let Some(clip) = self.clip.clone() else {
            return vec![row, col, row, col];
        };
        let (a, b) = self.sheet.paste(&clip, (row, col));
        vec![a.0, a.1, b.0, b.1]
    }

    /// A range as tab-separated text (what is shown on screen), for the
    /// system clipboard.
    pub fn range_to_tsv(&self, r1: u32, c1: u32, r2: u32, c2: u32) -> String {
        let (a, b) = rect(r1, c1, r2, c2);
        let field = |s: String| {
            if s.contains(['\t', '\n', '\r', '"']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s
            }
        };
        (a.0..=b.0)
            .map(|r| {
                (a.1..=b.1)
                    .map(|c| field(self.sheet.display((r, c))))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Paste tab-separated text (e.g. from another application) as typed
    /// input; returns the pasted area as `[r1, c1, r2, c2]`.
    pub fn paste_tsv(&mut self, row: u32, col: u32, text: &str) -> Vec<u32> {
        let rows = parse_delimited(text.trim_end_matches(['\r', '\n']), '\t');
        let height = rows.len() as u32;
        let width = rows.iter().map(|r| r.len() as u32).max().unwrap_or(0);
        self.sheet.batch(|s| {
            for (i, fields) in rows.iter().enumerate() {
                for (j, f) in fields.iter().enumerate() {
                    s.set_input((row + i as u32, col + j as u32), f);
                }
            }
        });
        vec![row, col, row + height.max(1) - 1, col + width.max(1) - 1]
    }

    pub fn fill_down(&mut self, r1: u32, c1: u32, r2: u32, c2: u32) {
        let (a, b) = rect(r1, c1, r2, c2);
        self.sheet.fill_down(a, b);
    }

    pub fn fill_right(&mut self, r1: u32, c1: u32, r2: u32, c2: u32) {
        let (a, b) = rect(r1, c1, r2, c2);
        self.sheet.fill_right(a, b);
    }

    // ---- structure ---------------------------------------------------------

    /// Insert `count` rows (`axis` "row") or columns ("col") before `at`.
    pub fn insert(&mut self, axis: &str, at: u32, count: u32) -> Result<(), JsError> {
        self.sheet.insert(axis_of(axis)?, at, count);
        Ok(())
    }

    pub fn delete(&mut self, axis: &str, at: u32, count: u32) -> Result<(), JsError> {
        self.sheet.delete(axis_of(axis)?, at, count);
        Ok(())
    }

    // ---- history and files ---------------------------------------------------

    pub fn undo(&mut self) {
        self.sheet.undo();
    }

    pub fn redo(&mut self) {
        self.sheet.redo();
    }

    pub fn can_undo(&self) -> bool {
        self.sheet.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.sheet.can_redo()
    }

    /// Changes whenever the sheet does (edits, undo, redo, resizing).
    pub fn revision(&self) -> u32 {
        self.sheet.revision() as u32
    }

    /// Forget undo/redo history; the current state becomes the starting point.
    pub fn clear_history(&mut self) {
        self.sheet.forget_history();
    }

    pub fn new_sheet(&mut self) {
        self.sheet.load(Default::default());
        self.clip = None;
    }

    pub fn load_csv(&mut self, text: &str) {
        self.sheet.load_csv(text);
        self.clip = None;
    }

    pub fn to_csv(&self) -> String {
        self.sheet.to_csv()
    }

    // ---- addresses -----------------------------------------------------------

    /// "B3" for a zero-based row/column.
    pub fn a1(&self, row: u32, col: u32) -> String {
        key_to_a1((row, col))
    }

    /// Parse "B3", "B3:D9", "B:B" or "3:3" into `[r1, c1, r2, c2]`; empty if invalid.
    pub fn parse_range(&self, text: &str) -> Vec<u32> {
        let text = text.trim();
        let (first, second) = text.split_once(':').unwrap_or((text, text));
        let cell = |s: &str| CellRef::parse(s).map(|r| (r.row, r.col));
        if let (Some(a), Some(b)) = (cell(first), cell(second)) {
            return vec![a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)];
        }
        if let (Some(a), Some(b)) = (quill_calc::parse_col(first), quill_calc::parse_col(second)) {
            return vec![0, a.min(b), MAX_ROWS - 1, a.max(b)];
        }
        if let (Ok(a), Ok(b)) = (first.parse::<u32>(), second.parse::<u32>()) {
            if (1..=MAX_ROWS).contains(&a) && (1..=MAX_ROWS).contains(&b) {
                return vec![a.min(b) - 1, 0, a.max(b) - 1, MAX_COLS - 1];
            }
        }
        vec![]
    }
}

fn axis_of(name: &str) -> Result<Axis, JsError> {
    match name {
        "row" => Ok(Axis::Row),
        "col" => Ok(Axis::Col),
        other => Err(js_err(format!("unknown axis: {other}"))),
    }
}

impl Default for SheetEditor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(cells: &[(u32, u32, &str)]) -> SheetEditor {
        let mut e = SheetEditor::new();
        for (r, c, t) in cells {
            e.set_input(*r, *c, t);
        }
        e
    }

    #[test]
    fn ctrl_arrow_runs_to_block_edges_and_between_blocks() {
        // A1:A3 filled, A4 empty, A5 filled, nothing after.
        let e = editor(&[(0, 0, "1"), (1, 0, "2"), (2, 0, "3"), (4, 0, "5")]);
        assert_eq!(e.edge(0, 0, 1, 0), vec![2, 0], "inside a block: to its end");
        assert_eq!(
            e.edge(2, 0, 1, 0),
            vec![4, 0],
            "at a block's end: to the next block"
        );
        assert_eq!(
            e.edge(4, 0, 1, 0),
            vec![MAX_ROWS - 1, 0],
            "no more data: to the last row"
        );
        assert_eq!(
            e.edge(3, 0, 1, 0),
            vec![4, 0],
            "from an empty cell: to the next filled one"
        );
        assert_eq!(e.edge(1, 0, -1, 0), vec![0, 0]);
        assert_eq!(
            e.edge(4, 0, -1, 0),
            vec![2, 0],
            "up from A5 lands on the end of the block above"
        );
        assert_eq!(e.edge(2, 0, -1, 0), vec![0, 0]);
        assert_eq!(e.edge(0, 0, -1, 0), vec![0, 0], "already at the edge");
        assert_eq!(e.edge(0, 0, 0, -1), vec![0, 0]);
        assert_eq!(
            e.edge(0, 0, 0, 1),
            vec![0, MAX_COLS - 1],
            "an empty row beyond the data: to the last column"
        );
        assert_eq!(
            e.edge(9, 3, -1, 0),
            vec![0, 3],
            "up an empty column goes to row 1"
        );
        assert_eq!(
            e.edge(5, 5, 0, -1),
            vec![5, 0],
            "left along an empty row goes to column A"
        );
    }

    #[test]
    fn ctrl_arrow_along_a_row_of_data() {
        let e = editor(&[(0, 0, "a"), (0, 1, "b"), (0, 3, "d")]);
        assert_eq!(e.edge(0, 0, 0, 1), vec![0, 1]);
        assert_eq!(e.edge(0, 1, 0, 1), vec![0, 3]);
        assert_eq!(e.edge(0, 3, 0, 1), vec![0, MAX_COLS - 1]);
        assert_eq!(e.edge(0, 3, 0, -1), vec![0, 1]);
    }

    #[test]
    fn ranges_parse_from_names_columns_and_rows() {
        let e = SheetEditor::new();
        assert_eq!(e.parse_range("B3"), vec![2, 1, 2, 1]);
        assert_eq!(e.parse_range(" b3:d9 "), vec![2, 1, 8, 3]);
        assert_eq!(
            e.parse_range("D9:B3"),
            vec![2, 1, 8, 3],
            "corners are normalised"
        );
        assert_eq!(e.parse_range("B:C"), vec![0, 1, MAX_ROWS - 1, 2]);
        assert_eq!(e.parse_range("2:4"), vec![1, 0, 3, MAX_COLS - 1]);
        assert_eq!(
            e.parse_range("A1048576"),
            vec![MAX_ROWS - 1, 0, MAX_ROWS - 1, 0]
        );
        for bad in [
            "",
            "A0",
            "1A",
            "A1:",
            ":B2",
            "ZZZZ",
            "0:3",
            "hello world",
            "A1:B",
        ] {
            assert!(e.parse_range(bad).is_empty(), "{bad:?}");
        }
        assert_eq!(e.a1(9, 27), "AB10");
    }

    #[test]
    fn clipboard_text_round_trips_what_is_shown() {
        let mut e = editor(&[
            (0, 0, "name"),
            (0, 1, "12%"),
            (1, 0, "a\tb"),
            (1, 1, "say \"hi\""),
            (2, 1, "=1+2"),
        ]);
        let tsv = e.range_to_tsv(0, 0, 2, 1);
        assert_eq!(tsv, "name\t12%\n\"a\tb\"\t\"say \"\"hi\"\"\"\n\t3");
        // Pasting it elsewhere reproduces the cells (values; formulas arrive as their results).
        let area = e.paste_tsv(10, 2, &tsv);
        assert_eq!(area, vec![10, 2, 12, 3]);
        assert_eq!(e.display(10, 2), "name");
        assert_eq!(e.display(10, 3), "12%");
        assert_eq!(e.raw(11, 2), "a\tb");
        assert_eq!(e.raw(11, 3), "say \"hi\"");
        assert_eq!(e.raw(12, 3), "3");
        e.undo();
        assert_eq!(e.raw(10, 2), "", "a paste is one undo step");
        // Single values and trailing newlines.
        assert_eq!(e.paste_tsv(0, 5, "42\n"), vec![0, 5, 0, 5]);
        assert_eq!(e.display(0, 5), "42");
        assert_eq!(
            e.paste_tsv(0, 6, "a, b; c"),
            vec![0, 6, 0, 6],
            "only tabs split fields"
        );
        assert_eq!(e.display(0, 6), "a, b; c");
    }

    #[test]
    fn internal_copy_keeps_formulas_and_shifts_them() {
        let mut e = editor(&[(0, 0, "2"), (0, 1, "=A1*3")]);
        e.copy(0, 1, 0, 1);
        assert!(e.has_clip());
        assert_eq!(e.paste(1, 1), vec![1, 1, 1, 1]);
        assert_eq!(e.raw(1, 1), "=A2*3");
        assert_eq!(
            SheetEditor::new().paste(4, 4),
            vec![4, 4, 4, 4],
            "nothing copied: nothing happens"
        );
    }

    #[test]
    fn decimals_and_number_formats() {
        let mut e = editor(&[(0, 0, "1234.5678")]);
        e.adjust_decimals(0, 0, 0, 0, 1);
        assert_eq!(e.display(0, 0), "1234.6", "general → one decimal");
        e.adjust_decimals(0, 0, 0, 0, 1);
        assert_eq!(e.display(0, 0), "1234.57");
        e.adjust_decimals(0, 0, 0, 0, -1);
        e.adjust_decimals(0, 0, 0, 0, -1);
        e.adjust_decimals(0, 0, 0, 0, -1);
        assert_eq!(e.display(0, 0), "1235", "never below zero decimals");
        e.style(0, 0, 0, 0, "numfmt", "currency")
            .map_err(|_| ())
            .unwrap();
        assert_eq!(e.display(0, 0), "$1,234.57");
        e.style(0, 0, 0, 0, "numfmt", "percent")
            .map_err(|_| ())
            .unwrap();
        assert_eq!(e.display(0, 0), "123457%");
        e.style(0, 0, 0, 0, "numfmt", "general")
            .map_err(|_| ())
            .unwrap();
        assert_eq!(e.display(0, 0), "1234.5678");
    }

    #[test]
    fn structure_and_history_pass_through() {
        let mut e = editor(&[(0, 0, "1"), (1, 0, "=A1+1")]);
        e.insert("row", 0, 1).map_err(|_| ()).unwrap();
        assert_eq!(e.raw(2, 0), "=A2+1");
        e.delete("col", 5, 1).map_err(|_| ()).unwrap();
        assert!(e.can_undo());
        e.undo();
        e.undo();
        assert_eq!(e.raw(1, 0), "=A1+1");
        assert!(e.can_redo());
        assert_eq!(e.used_range(), vec![2, 1]);
        let r = e.revision();
        e.set_col_width(2, 180.0);
        assert!(e.revision() > r);
        e.load_csv("x,y\n1,2");
        assert_eq!(e.to_csv(), "x,y\n1,2\n");
        assert!(!e.can_undo());
        e.new_sheet();
        assert_eq!(e.used_range(), vec![0, 0]);
    }
}
