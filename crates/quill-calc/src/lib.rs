//! Quill's spreadsheet engine: a sparse grid with formulas.

mod csv;
mod eval;
pub mod expr;
pub mod refs;
pub mod sheet;
pub mod style;

#[cfg(test)]
mod tests;
pub mod value;

pub use csv::{parse_csv, parse_delimited};
pub use expr::{parse, Axis, BinOp, Dep, Expr};
pub use refs::{col_name, key_to_a1, parse_col, CellRef, Key, MAX_COLS, MAX_ROWS};
pub use sheet::{parse_number, Cell, CellData, Clip, Sheet, DEFAULT_COL_WIDTH, DEFAULT_ROW_HEIGHT};
pub use style::{format_number, Align, NumFmt, Style};
pub use value::{format_fixed, format_general, round_half_away, ErrorKind, Value};
