//! The sheet: a sparse grid of cells, dependency tracking, edits and undo.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use crate::expr::{parse, Axis, Dep, Expr};
use crate::refs::{Key, MAX_COLS, MAX_ROWS};
use crate::style::{format_number, NumFmt, Style};
use crate::value::{ErrorKind, Value};

/// What a cell holds apart from its computed value: enough to rebuild it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CellData {
    /// The text as typed (a formula starts with `=`).
    pub raw: String,
    pub style: Style,
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub data: CellData,
    pub(crate) formula: Option<Rc<Expr>>,
    pub(crate) value: Value,
}

/// Cells copied from a range, ready to paste elsewhere.
#[derive(Clone, Debug)]
pub struct Clip {
    /// Where the copy came from (top-left), so pasted formulas can shift.
    pub origin: Key,
    /// Rows × columns; `None` is an empty cell.
    pub cells: Vec<Vec<Option<CellData>>>,
}

impl Clip {
    pub fn rows(&self) -> usize {
        self.cells.len()
    }

    pub fn cols(&self) -> usize {
        self.cells.first().map_or(0, Vec::len)
    }
}

type Patch = Vec<(Key, Option<CellData>)>;

enum Undo {
    /// Previous contents of the cells an edit touched.
    Cells(Patch),
    /// Previous state of the whole sheet (structural edits).
    Whole(
        BTreeMap<Key, CellData>,
        BTreeMap<u32, f32>,
        BTreeMap<u32, f32>,
    ),
}

pub const DEFAULT_COL_WIDTH: f32 = 100.0;
pub const DEFAULT_ROW_HEIGHT: f32 = 24.0;

#[derive(Default)]
pub struct Sheet {
    pub(crate) cells: BTreeMap<Key, Cell>,
    /// cell → formulas that read it directly.
    readers: HashMap<Key, HashSet<Key>>,
    /// formula → the ranges it reads (looked up by scanning on change).
    range_readers: HashMap<Key, Vec<(Key, Key)>>,
    /// formula → the single cells it reads (to unregister).
    reads: HashMap<Key, Vec<Key>>,
    pub(crate) stale: HashSet<Key>,
    pub(crate) evaluating: HashSet<Key>,
    /// Upper bound on the used column (never shrinks until a rebuild).
    pub(crate) max_col: u32,
    col_widths: BTreeMap<u32, f32>,
    row_heights: BTreeMap<u32, f32>,
    undo: Vec<Undo>,
    redo: Vec<Undo>,
    txn: Option<Patch>,
    revision: u64,
}

/// Interpret typed text as a number: `1,234.5`, `-3`, `$12`, `15%`, `1e3`.
pub fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    let (t, percent) = match t.strip_suffix('%') {
        Some(rest) => (rest.trim_end(), true),
        None => (t, false),
    };
    let (sign, t) = match t.as_bytes().first()? {
        b'-' => (-1.0, &t[1..]),
        b'+' => (1.0, &t[1..]),
        _ => (1.0, t),
    };
    let t = t.strip_prefix('$').unwrap_or(t);
    if t.is_empty() || !t.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    // Thousands separators are only accepted in their proper places.
    let (int_part, rest) = match t.find(['.', 'e', 'E']) {
        Some(i) => t.split_at(i),
        None => (t, ""),
    };
    let cleaned = if int_part.contains(',') {
        let groups: Vec<&str> = int_part.split(',').collect();
        let ok = !groups[0].is_empty()
            && groups[0].len() <= 3
            && groups[1..].iter().all(|g| g.len() == 3)
            && groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_digit()));
        if !ok {
            return None;
        }
        format!("{}{rest}", groups.concat())
    } else {
        t.to_string()
    };
    if !cleaned
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return None;
    }
    let n: f64 = cleaned.parse().ok()?;
    n.is_finite()
        .then(|| sign * if percent { n / 100.0 } else { n })
}

impl Sheet {
    pub fn new() -> Sheet {
        Sheet::default()
    }

    // ---- reading -----------------------------------------------------------

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The computed value of a cell (`Empty` for unused cells).
    pub fn value(&self, key: Key) -> Value {
        self.cells
            .get(&key)
            .map_or(Value::Empty, |c| c.value.clone())
    }

    /// What the user typed (formulas start with `=`).
    pub fn raw(&self, key: Key) -> String {
        self.cells
            .get(&key)
            .map_or_else(String::new, |c| c.data.raw.clone())
    }

    pub fn style(&self, key: Key) -> Style {
        self.cells
            .get(&key)
            .map_or_else(Style::default, |c| c.data.style.clone())
    }

    /// The cell as it should appear on screen.
    pub fn display(&self, key: Key) -> String {
        let Some(c) = self.cells.get(&key) else {
            return String::new();
        };
        match &c.value {
            Value::Number(n) => format_number(*n, c.data.style.fmt),
            other => other.to_text(),
        }
    }

    pub fn is_formula(&self, key: Key) -> bool {
        self.cells.get(&key).is_some_and(|c| c.formula.is_some())
    }

    /// Cells that hold something (content or formatting), in row-major order.
    pub fn used_cells(&self) -> impl Iterator<Item = (Key, &Cell)> {
        self.cells.iter().map(|(k, c)| (*k, c))
    }

    /// `(rows, cols)` needed to show every non-empty cell.
    pub fn used_range(&self) -> (u32, u32) {
        let rows = self.cells.keys().next_back().map_or(0, |k| k.0 + 1);
        let cols = self.cells.keys().map(|k| k.1 + 1).max().unwrap_or(0);
        (rows, cols)
    }

    pub fn col_width(&self, col: u32) -> f32 {
        self.col_widths
            .get(&col)
            .copied()
            .unwrap_or(DEFAULT_COL_WIDTH)
    }

    pub fn row_height(&self, row: u32) -> f32 {
        self.row_heights
            .get(&row)
            .copied()
            .unwrap_or(DEFAULT_ROW_HEIGHT)
    }

    pub fn set_col_width(&mut self, col: u32, width: f32) {
        self.col_widths.insert(col, width.clamp(20.0, 800.0));
        self.revision += 1;
    }

    pub fn set_row_height(&mut self, row: u32, height: f32) {
        self.row_heights.insert(row, height.clamp(12.0, 400.0));
        self.revision += 1;
    }

    // ---- editing primitives --------------------------------------------------

    fn unregister(&mut self, key: Key) {
        for r in self.reads.remove(&key).unwrap_or_default() {
            if let Some(set) = self.readers.get_mut(&r) {
                set.remove(&key);
                if set.is_empty() {
                    self.readers.remove(&r);
                }
            }
        }
        self.range_readers.remove(&key);
    }

    fn register(&mut self, key: Key, expr: &Expr) {
        let mut deps = Vec::new();
        expr.deps(&mut deps);
        let (mut singles, mut ranges) = (Vec::new(), Vec::new());
        for d in deps {
            match d {
                Dep::Cell(k) => singles.push(k),
                Dep::Range(a, b) => ranges.push((a, b)),
            }
        }
        for k in &singles {
            self.readers.entry(*k).or_default().insert(key);
        }
        self.reads.insert(key, singles);
        if !ranges.is_empty() {
            self.range_readers.insert(key, ranges);
        }
    }

    /// Mark every formula that (transitively) reads `key` as needing recalculation.
    fn invalidate_readers_of(&mut self, key: Key) {
        let mut queue = vec![key];
        while let Some(k) = queue.pop() {
            let mut hit: Vec<Key> = self
                .readers
                .get(&k)
                .map(|s| s.iter().copied().collect())
                .unwrap_or_default();
            for (f, ranges) in &self.range_readers {
                if ranges
                    .iter()
                    .any(|(a, b)| a.0 <= k.0 && k.0 <= b.0 && a.1 <= k.1 && k.1 <= b.1)
                {
                    hit.push(*f);
                }
            }
            for f in hit {
                if self.stale.insert(f) {
                    queue.push(f);
                }
            }
        }
    }

    /// Build a cell from typed text, applying number/percent/currency conventions.
    fn build(&self, data: CellData) -> Cell {
        let raw_owned = data.raw.clone();
        let raw = raw_owned.as_str();
        if let Some(body) = raw.strip_prefix('=').filter(|b| !b.trim().is_empty()) {
            return match parse(body) {
                Ok(e) => Cell {
                    data,
                    formula: Some(Rc::new(e)),
                    value: Value::Empty,
                },
                Err(_) => Cell {
                    data,
                    formula: None,
                    value: Value::Error(ErrorKind::Parse),
                },
            };
        }
        let mut data = data;
        let value = if let Some(text) = raw.strip_prefix('\'') {
            Value::Text(text.to_string())
        } else if raw.trim().is_empty() {
            Value::Empty
        } else if raw.trim().eq_ignore_ascii_case("true") {
            Value::Bool(true)
        } else if raw.trim().eq_ignore_ascii_case("false") {
            Value::Bool(false)
        } else if let Some(e) = ErrorKind::from_code(raw.trim()) {
            Value::Error(e)
        } else if let Some(n) = parse_number(raw) {
            // Typing "12%" or "$5" also picks the matching display format.
            if data.style.fmt == NumFmt::General {
                let t = raw.trim();
                let decimals = |s: &str| {
                    s.split_once('.')
                        .map_or(0, |(_, f)| f.trim_end_matches('%').len().min(10) as u8)
                };
                if t.ends_with('%') {
                    data.style.fmt = NumFmt::Percent {
                        decimals: decimals(t),
                    };
                } else if t.trim_start_matches(['-', '+']).starts_with('$') {
                    data.style.fmt = NumFmt::Currency {
                        decimals: decimals(t).max(if t.contains('.') { 0 } else { 2 }),
                    };
                }
            }
            Value::Number(n)
        } else {
            Value::Text(raw.to_string())
        };
        Cell {
            data,
            formula: None,
            value,
        }
    }

    /// Replace one cell (or clear it with `None`), recording the old state for undo.
    fn put(&mut self, key: Key, new: Option<CellData>) {
        if key.0 >= MAX_ROWS || key.1 >= MAX_COLS {
            return;
        }
        let old = self.cells.get(&key).map(|c| c.data.clone());
        let new = new.filter(|d| !(d.raw.is_empty() && d.style.is_default()));
        if old == new {
            return;
        }
        if let Some(txn) = self.txn.as_mut() {
            if !txn.iter().any(|(k, _)| *k == key) {
                txn.push((key, old));
            }
        }
        self.unregister(key);
        self.stale.remove(&key);
        match new {
            Some(data) => {
                let cell = self.build(data);
                if let Some(f) = cell.formula.clone() {
                    self.register(key, &f);
                    self.stale.insert(key);
                }
                self.max_col = self.max_col.max(key.1);
                self.cells.insert(key, cell);
            }
            None => {
                self.cells.remove(&key);
            }
        }
        self.invalidate_readers_of(key);
    }

    /// Evaluate everything that is out of date.
    fn recalc(&mut self) {
        let mut stale: Vec<Key> = self.stale.iter().copied().collect();
        stale.sort_unstable();
        for k in stale {
            if self.stale.contains(&k) {
                self.compute(k);
            }
        }
        self.stale.clear();
    }

    /// Run `f` as one undoable step.
    pub fn batch(&mut self, f: impl FnOnce(&mut Sheet)) {
        let nested = self.txn.is_some();
        if !nested {
            self.txn = Some(Vec::new());
        }
        f(self);
        if nested {
            return;
        }
        self.recalc();
        let patch = self.txn.take().unwrap_or_default();
        if !patch.is_empty() {
            self.undo.push(Undo::Cells(patch));
            self.redo.clear();
            self.revision += 1;
        }
    }

    // ---- public edits --------------------------------------------------------

    /// Type `raw` into a cell, keeping its formatting.
    pub fn set_input(&mut self, key: Key, raw: &str) {
        self.batch(|s| {
            let style = s.style(key);
            s.put(
                key,
                Some(CellData {
                    raw: raw.to_string(),
                    style,
                }),
            );
        });
    }

    /// Clear the contents of a range, keeping formatting.
    pub fn clear_contents(&mut self, a: Key, b: Key) {
        self.batch(|s| {
            for key in s.keys_in(a, b) {
                let style = s.style(key);
                s.put(
                    key,
                    Some(CellData {
                        raw: String::new(),
                        style,
                    }),
                );
            }
        });
    }

    /// Clear contents and formatting.
    pub fn clear_all(&mut self, a: Key, b: Key) {
        self.batch(|s| {
            for key in s.keys_in(a, b) {
                s.put(key, None);
            }
        });
    }

    /// Change the formatting of every cell in a range (including empty ones).
    pub fn style_range(&mut self, a: Key, b: Key, f: impl Fn(&mut Style)) {
        let (r1, r2, c1, c2) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
        // Whole-row/column selections only style cells that exist plus a bounded area.
        let cap = 20_000u64;
        let area = (r2 - r1 + 1) as u64 * (c2 - c1 + 1) as u64;
        self.batch(|s| {
            let keys: Vec<Key> = if area <= cap {
                (r1..=r2)
                    .flat_map(|r| (c1..=c2).map(move |c| (r, c)))
                    .collect()
            } else {
                s.keys_in((r1, c1), (r2, c2))
            };
            for key in keys {
                let mut d = s
                    .cells
                    .get(&key)
                    .map(|c| c.data.clone())
                    .unwrap_or_default();
                f(&mut d.style);
                s.put(key, Some(d));
            }
        });
    }

    /// Existing cells within the rectangle spanned by `a` and `b`, row-major.
    pub fn cells_in(&self, a: Key, b: Key) -> Vec<(Key, &Cell)> {
        self.keys_in(a, b)
            .into_iter()
            .filter_map(|k| self.cells.get(&k).map(|c| (k, c)))
            .collect()
    }

    /// Existing cells within the rectangle spanned by `a` and `b`.
    pub(crate) fn keys_in(&self, a: Key, b: Key) -> Vec<Key> {
        let (r1, r2, c1, c2) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
        self.cells
            .range((r1, 0)..=(r2, u32::MAX))
            .map(|(k, _)| *k)
            .filter(|k| (c1..=c2).contains(&k.1))
            .collect()
    }

    // ---- copy, paste, fill ---------------------------------------------------

    pub fn copy_range(&self, a: Key, b: Key) -> Clip {
        let (r1, r2, c1, c2) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
        let cells = (r1..=r2)
            .map(|r| {
                (c1..=c2)
                    .map(|c| self.cells.get(&(r, c)).map(|cell| cell.data.clone()))
                    .collect()
            })
            .collect();
        Clip {
            origin: (r1, c1),
            cells,
        }
    }

    /// `data` as if moved by `(dr, dc)`: relative references in formulas shift.
    fn moved(data: &CellData, dr: i64, dc: i64) -> CellData {
        let Some(body) = data.raw.strip_prefix('=') else {
            return data.clone();
        };
        match parse(body) {
            Ok(e) => CellData {
                raw: format!("={}", e.shifted(dr, dc).to_formula()),
                style: data.style.clone(),
            },
            Err(_) => data.clone(),
        }
    }

    /// Paste with its top-left at `at`; returns the area that was written.
    pub fn paste(&mut self, clip: &Clip, at: Key) -> (Key, Key) {
        let (dr, dc) = (
            at.0 as i64 - clip.origin.0 as i64,
            at.1 as i64 - clip.origin.1 as i64,
        );
        self.batch(|s| {
            for (i, row) in clip.cells.iter().enumerate() {
                for (j, cell) in row.iter().enumerate() {
                    let key = (at.0 + i as u32, at.1 + j as u32);
                    s.put(key, cell.as_ref().map(|d| Sheet::moved(d, dr, dc)));
                }
            }
        });
        (
            at,
            (
                at.0 + clip.rows().max(1) as u32 - 1,
                at.1 + clip.cols().max(1) as u32 - 1,
            ),
        )
    }

    /// Copy the top row of the range down through the rest of it.
    pub fn fill_down(&mut self, a: Key, b: Key) {
        let (r1, r2, c1, c2) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
        self.batch(|s| {
            for c in c1..=c2 {
                let src = s.cells.get(&(r1, c)).map(|x| x.data.clone());
                for r in r1 + 1..=r2 {
                    s.put(
                        (r, c),
                        src.as_ref().map(|d| Sheet::moved(d, (r - r1) as i64, 0)),
                    );
                }
            }
        });
    }

    /// Copy the left column of the range across the rest of it.
    pub fn fill_right(&mut self, a: Key, b: Key) {
        let (r1, r2, c1, c2) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
        self.batch(|s| {
            for r in r1..=r2 {
                let src = s.cells.get(&(r, c1)).map(|x| x.data.clone());
                for c in c1 + 1..=c2 {
                    s.put(
                        (r, c),
                        src.as_ref().map(|d| Sheet::moved(d, 0, (c - c1) as i64)),
                    );
                }
            }
        });
    }

    // ---- structure -----------------------------------------------------------

    fn snapshot(&self) -> Undo {
        Undo::Whole(
            self.cells
                .iter()
                .map(|(k, c)| (*k, c.data.clone()))
                .collect(),
            self.col_widths.clone(),
            self.row_heights.clone(),
        )
    }

    /// Replace the whole grid from raw cell data and recalculate.
    fn rebuild(&mut self, cells: BTreeMap<Key, CellData>) {
        self.cells.clear();
        self.readers.clear();
        self.range_readers.clear();
        self.reads.clear();
        self.stale.clear();
        self.max_col = 0;
        for (key, data) in cells {
            let cell = self.build(data);
            if let Some(f) = cell.formula.clone() {
                self.register(key, &f);
                self.stale.insert(key);
            }
            self.max_col = self.max_col.max(key.1);
            self.cells.insert(key, cell);
        }
        self.recalc();
    }

    fn restructure(&mut self, axis: Axis, at: u32, count: u32, insert: bool) {
        if count == 0 {
            return;
        }
        self.undo.push(self.snapshot());
        self.redo.clear();
        let limit = if axis == Axis::Row {
            MAX_ROWS
        } else {
            MAX_COLS
        };
        let mut moved = BTreeMap::new();
        for (key, c) in std::mem::take(&mut self.cells) {
            let idx = if axis == Axis::Row { key.0 } else { key.1 };
            let new_idx = if insert {
                Some(if idx >= at { idx + count } else { idx })
            } else if idx < at {
                Some(idx)
            } else if idx >= at + count {
                Some(idx - count)
            } else {
                None
            };
            let Some(n) = new_idx.filter(|n| *n < limit) else {
                continue;
            };
            let nk = if axis == Axis::Row {
                (n, key.1)
            } else {
                (key.0, n)
            };
            // Formulas everywhere follow the move.
            let mut data = c.data;
            if let Some(body) = data.raw.strip_prefix('=') {
                if let Ok(e) = parse(body) {
                    let e = if insert {
                        e.after_insert(axis, at, count)
                    } else {
                        e.after_delete(axis, at, count)
                    };
                    data.raw = format!("={}", e.to_formula());
                }
            }
            moved.insert(nk, data);
        }
        let sizes = if axis == Axis::Row {
            &mut self.row_heights
        } else {
            &mut self.col_widths
        };
        *sizes = std::mem::take(sizes)
            .into_iter()
            .filter_map(|(i, v)| {
                let n = if insert {
                    Some(if i >= at { i + count } else { i })
                } else if i < at {
                    Some(i)
                } else if i >= at + count {
                    Some(i - count)
                } else {
                    None
                };
                n.map(|n| (n, v))
            })
            .collect();
        self.rebuild(moved);
        self.revision += 1;
    }

    /// Insert `count` empty rows/columns before index `at`.
    pub fn insert(&mut self, axis: Axis, at: u32, count: u32) {
        self.restructure(axis, at, count, true);
    }

    /// Delete `count` rows/columns starting at `at`.
    pub fn delete(&mut self, axis: Axis, at: u32, count: u32) {
        self.restructure(axis, at, count, false);
    }

    // ---- undo ----------------------------------------------------------------

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Apply an undo record and return the record that reverses it.
    fn apply(&mut self, u: Undo) -> Undo {
        match u {
            Undo::Cells(patch) => {
                self.txn = Some(Vec::new());
                for (key, data) in patch {
                    self.put(key, data);
                }
                self.recalc();
                Undo::Cells(self.txn.take().unwrap_or_default())
            }
            Undo::Whole(cells, cw, rh) => {
                let inverse = self.snapshot();
                self.col_widths = cw;
                self.row_heights = rh;
                self.rebuild(cells);
                inverse
            }
        }
    }

    pub fn undo(&mut self) {
        if let Some(u) = self.undo.pop() {
            let inverse = self.apply(u);
            self.redo.push(inverse);
            self.revision += 1;
        }
    }

    pub fn redo(&mut self) {
        if let Some(u) = self.redo.pop() {
            let inverse = self.apply(u);
            self.undo.push(inverse);
            self.revision += 1;
        }
    }

    /// Forget undo/redo history and make the current state revision 0
    /// (e.g. after building a starting document).
    pub fn forget_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.revision = 0;
    }

    /// Replace everything (e.g. when loading a file); clears history.
    pub fn load(&mut self, cells: BTreeMap<Key, CellData>) {
        self.col_widths.clear();
        self.row_heights.clear();
        self.rebuild(cells);
        self.undo.clear();
        self.redo.clear();
        self.revision = 0;
    }

    /// Every cell's typed data, for saving.
    pub fn export(&self) -> BTreeMap<Key, CellData> {
        self.cells
            .iter()
            .map(|(k, c)| (*k, c.data.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_recognised_in_common_notations() {
        for (s, n) in [
            ("42", 42.0),
            (" -3.5 ", -3.5),
            ("1,234.5", 1234.5),
            ("$12", 12.0),
            ("-$1,000", -1000.0),
            ("15%", 0.15),
            ("1e3", 1000.0),
            (".5", 0.5),
            ("+7", 7.0),
            ("1,234,567", 1234567.0),
            ("2.5E-3", 0.0025),
        ] {
            assert_eq!(parse_number(s), Some(n), "{s}");
        }
        for s in [
            "",
            "abc",
            "1,23",
            "12,34,567",
            ",123",
            "1.2.3",
            "inf",
            "NaN",
            "infinity",
            "1e",
            "$",
            "--1",
            "12abc",
            "1 2",
            "0x10",
            "%",
        ] {
            assert_eq!(parse_number(s), None, "{s:?}");
        }
    }
}
