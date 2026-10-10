//! CSV import and export.

use std::collections::BTreeMap;

use crate::refs::Key;
use crate::sheet::{CellData, Sheet};
use crate::value::Value;

/// Pick the delimiter that appears most often in the first line (outside quotes).
fn detect_delimiter(text: &str) -> char {
    let mut counts = [(',', 0usize), (';', 0), ('\t', 0)];
    let mut in_quotes = false;
    for c in text.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            '\n' if !in_quotes => break,
            c if !in_quotes => {
                if let Some(e) = counts.iter_mut().find(|(d, _)| *d == c) {
                    e.1 += 1;
                }
            }
            _ => {}
        }
    }
    counts
        .iter()
        .max_by_key(|(_, n)| *n)
        .filter(|(_, n)| *n > 0)
        .map_or(',', |(d, _)| *d)
}

/// Split CSV text into rows of fields (RFC 4180 quoting; CRLF, CR or LF line ends).
/// The delimiter (comma, semicolon or tab) is detected from the first line.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    parse_delimited(text, detect_delimiter(text))
}

/// Like [`parse_csv`] with a fixed delimiter (e.g. a tab for clipboard text).
pub fn parse_delimited(text: &str, delim: char) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut rows = Vec::new();
    let (mut row, mut field) = (Vec::new(), String::new());
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    // A row is real if it has any field, or the file didn't end right after a newline.
    let mut pending = false;
    while let Some(c) = chars.next() {
        pending = true;
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => in_quotes = false,
                c => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => in_quotes = true,
            c if c == delim => row.push(std::mem::take(&mut field)),
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                pending = false;
            }
            c => field.push(c),
        }
    }
    if pending {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn quote(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

impl Sheet {
    /// Replace the sheet with the contents of CSV text (history is cleared).
    pub fn load_csv(&mut self, text: &str) {
        let mut cells: BTreeMap<Key, CellData> = BTreeMap::new();
        for (r, row) in parse_csv(text).into_iter().enumerate() {
            for (c, field) in row.into_iter().enumerate() {
                if !field.is_empty() {
                    cells.insert(
                        (r as u32, c as u32),
                        CellData {
                            raw: field,
                            ..Default::default()
                        },
                    );
                }
            }
        }
        self.load(cells);
    }

    /// The sheet's computed values as CSV. Text that a spreadsheet would read
    /// back as a formula (starting with `=` or `@`) gets a leading `'`.
    pub fn to_csv(&self) -> String {
        let (rows, cols) = self.used_range();
        let mut out = String::new();
        for r in 0..rows {
            let line: Vec<String> = (0..cols)
                .map(|c| match self.value((r, c)) {
                    Value::Text(t) if t.starts_with(['=', '@']) => quote(&format!("'{t}")),
                    v => quote(&v.to_text()),
                })
                .collect();
            out.push_str(line.join(",").trim_end_matches(','));
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(s: &str) -> Vec<Vec<String>> {
        parse_csv(s)
    }

    #[test]
    fn parses_quotes_newlines_and_line_endings() {
        assert_eq!(rows("a,b\n1,2\n"), vec![vec!["a", "b"], vec!["1", "2"]]);
        assert_eq!(rows("a,b\r\n1,2"), vec![vec!["a", "b"], vec!["1", "2"]]);
        assert_eq!(rows("a\rb\r"), vec![vec!["a"], vec!["b"]]);
        assert_eq!(
            rows("\"x,y\",\"say \"\"hi\"\"\",\"two\nlines\"\n"),
            vec![vec!["x,y", "say \"hi\"", "two\nlines"]]
        );
        assert_eq!(
            rows("a,,c\n,,\n"),
            vec![vec!["a", "", "c"], vec!["", "", ""]]
        );
        assert_eq!(rows(""), Vec::<Vec<String>>::new());
        assert_eq!(rows("\u{FEFF}a,b"), vec![vec!["a", "b"]], "BOM is dropped");
        assert_eq!(
            rows("a,\"unterminated"),
            vec![vec!["a", "unterminated"]],
            "tolerates a missing closing quote"
        );
        assert_eq!(
            rows("x\n\ny"),
            vec![vec!["x"], vec![""], vec!["y"]],
            "blank lines are kept as empty rows"
        );
    }

    #[test]
    fn a_fixed_delimiter_ignores_other_separators() {
        assert_eq!(
            parse_delimited("a, b\tc;d\n1\t2", '\t'),
            vec![vec!["a, b", "c;d"], vec!["1", "2"]]
        );
        assert_eq!(
            parse_delimited("just one, line", '\t'),
            vec![vec!["just one, line"]]
        );
        assert_eq!(
            parse_delimited("\"x\ty\"\tz", '\t'),
            vec![vec!["x\ty", "z"]]
        );
    }

    #[test]
    fn detects_semicolon_and_tab_delimiters() {
        assert_eq!(
            rows("a;b;c\n1,5;2;3"),
            vec![vec!["a", "b", "c"], vec!["1,5", "2", "3"]]
        );
        assert_eq!(rows("a\tb\n1\t2"), vec![vec!["a", "b"], vec!["1", "2"]]);
        assert_eq!(
            rows("\"a;b\",c\n1,2"),
            vec![vec!["a;b", "c"], vec!["1", "2"]],
            "quoted delimiters don't count"
        );
        assert_eq!(rows("single\nvalues"), vec![vec!["single"], vec!["values"]]);
    }

    #[test]
    fn loads_values_and_formulas_and_exports_computed_results() {
        let mut s = Sheet::new();
        s.load_csv("Item,Qty,Price,Total\nPen,2,1.5,=B2*C2\nBook,1,12,=B3*C3\n,,Sum,=SUM(D2:D3)\n");
        assert_eq!(s.value((1, 3)), Value::Number(3.0));
        assert_eq!(s.value((3, 3)), Value::Number(15.0));
        assert_eq!(
            s.to_csv(),
            "Item,Qty,Price,Total\nPen,2,1.5,3\nBook,1,12,12\n,,Sum,15\n"
        );
        assert!(!s.can_undo(), "loading starts with fresh history");
    }

    #[test]
    fn export_quotes_fields_and_defuses_formula_text() {
        let mut s = Sheet::new();
        s.set_input((0, 0), "a,b");
        s.set_input((0, 1), "say \"hi\"");
        s.set_input((1, 0), "'=1+1"); // text that looks like a formula
        s.set_input((1, 1), "line1");
        s.set_input((2, 0), "=A1&\"!\"");
        s.set_input((2, 1), "'@home");
        let csv = s.to_csv();
        assert_eq!(
            csv,
            "\"a,b\",\"say \"\"hi\"\"\"\n'=1+1,line1\n\"a,b!\",'@home\n"
        );
        // Round trip: the apostrophe marks the text as text when read back.
        let mut back = Sheet::new();
        back.load_csv(&csv);
        assert_eq!(back.value((1, 0)), Value::Text("=1+1".into()));
        assert_eq!(back.value((0, 0)), Value::Text("a,b".into()));
    }

    #[test]
    fn export_of_an_empty_sheet_and_errors() {
        assert_eq!(Sheet::new().to_csv(), "");
        let mut s = Sheet::new();
        s.set_input((0, 0), "=1/0");
        s.set_input((0, 2), "x");
        assert_eq!(s.to_csv(), "#DIV/0!,,x\n");
    }
}
