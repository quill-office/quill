//! Cell references in A1 notation.

pub const MAX_ROWS: u32 = 1_048_576;
pub const MAX_COLS: u32 = 16_384;

/// A cell's position as `(row, col)`, both zero-based.
pub type Key = (u32, u32);

/// Column letters for a zero-based index: 0 → "A", 26 → "AA".
pub fn col_name(col: u32) -> String {
    let mut n = col + 1;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).expect("ASCII letters")
}

/// Zero-based index for column letters ("A" → 0), or `None` if invalid or out of range.
pub fn parse_col(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 3 || !s.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let n = s.bytes().fold(0u32, |acc, b| {
        acc * 26 + (b.to_ascii_uppercase() - b'A' + 1) as u32
    });
    (n <= MAX_COLS).then(|| n - 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellRef {
    pub row: u32,
    pub col: u32,
    pub abs_row: bool,
    pub abs_col: bool,
}

impl CellRef {
    pub fn new(row: u32, col: u32) -> CellRef {
        CellRef {
            row,
            col,
            abs_row: false,
            abs_col: false,
        }
    }

    pub fn key(&self) -> Key {
        (self.row, self.col)
    }

    /// Parse "A1", "$B$2", "c3" …
    pub fn parse(s: &str) -> Option<CellRef> {
        let mut rest = s;
        let abs_col = rest.starts_with('$');
        if abs_col {
            rest = &rest[1..];
        }
        let letters = rest.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
        let (col_s, mut rest) = rest.split_at(letters);
        let col = parse_col(col_s)?;
        let abs_row = rest.starts_with('$');
        if abs_row {
            rest = &rest[1..];
        }
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let row: u32 = rest.parse().ok()?;
        (1..=MAX_ROWS).contains(&row).then(|| CellRef {
            row: row - 1,
            col,
            abs_row,
            abs_col,
        })
    }

    pub fn to_a1(&self) -> String {
        format!(
            "{}{}{}{}",
            if self.abs_col { "$" } else { "" },
            col_name(self.col),
            if self.abs_row { "$" } else { "" },
            self.row + 1
        )
    }

    /// Move a relative reference by `(dr, dc)`; `None` if it would leave the sheet.
    /// Absolute parts stay put.
    pub fn shifted(&self, dr: i64, dc: i64) -> Option<CellRef> {
        let row = if self.abs_row {
            self.row as i64
        } else {
            self.row as i64 + dr
        };
        let col = if self.abs_col {
            self.col as i64
        } else {
            self.col as i64 + dc
        };
        ((0..MAX_ROWS as i64).contains(&row) && (0..MAX_COLS as i64).contains(&col)).then_some(
            CellRef {
                row: row as u32,
                col: col as u32,
                ..*self
            },
        )
    }
}

/// "A1" for a key, e.g. for a name box.
pub fn key_to_a1(key: Key) -> String {
    format!("{}{}", col_name(key.1), key.0 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_letters_round_trip() {
        for (i, n) in [
            (0, "A"),
            (25, "Z"),
            (26, "AA"),
            (27, "AB"),
            (701, "ZZ"),
            (702, "AAA"),
            (16383, "XFD"),
        ] {
            assert_eq!(col_name(i), n);
            assert_eq!(parse_col(n), Some(i));
        }
        assert_eq!(parse_col("xfd"), Some(16383));
        assert_eq!(parse_col("XFE"), None, "past the last column");
        assert_eq!(parse_col(""), None);
        assert_eq!(parse_col("A1"), None);
        assert_eq!(parse_col("ABCD"), None);
    }

    #[test]
    fn a1_references_parse_and_print() {
        let r = CellRef::parse("$B$12").unwrap();
        assert_eq!((r.row, r.col, r.abs_row, r.abs_col), (11, 1, true, true));
        assert_eq!(r.to_a1(), "$B$12");
        assert_eq!(CellRef::parse("c3").unwrap().to_a1(), "C3");
        assert_eq!(CellRef::parse("A$1").unwrap().to_a1(), "A$1");
        assert_eq!(CellRef::parse("$A1").unwrap().to_a1(), "$A1");
        for bad in [
            "", "A", "1", "A0", "A1048577", "1A", "A1B", "$", "$$A1", "A$",
        ] {
            assert!(CellRef::parse(bad).is_none(), "{bad}");
        }
        assert!(CellRef::parse("A1048576").is_some());
        assert_eq!(key_to_a1((9, 27)), "AB10");
    }

    #[test]
    fn shifting_respects_absolute_parts_and_sheet_bounds() {
        let r = CellRef::parse("B2").unwrap();
        assert_eq!(r.shifted(1, 2).unwrap().to_a1(), "D3");
        assert_eq!(
            CellRef::parse("$B$2")
                .unwrap()
                .shifted(5, 5)
                .unwrap()
                .to_a1(),
            "$B$2"
        );
        assert_eq!(
            CellRef::parse("$B2")
                .unwrap()
                .shifted(1, 1)
                .unwrap()
                .to_a1(),
            "$B3"
        );
        assert_eq!(
            CellRef::parse("B$2")
                .unwrap()
                .shifted(1, 1)
                .unwrap()
                .to_a1(),
            "C$2"
        );
        assert!(r.shifted(-2, 0).is_none(), "above the first row");
        assert!(r.shifted(0, -2).is_none(), "left of column A");
    }
}
