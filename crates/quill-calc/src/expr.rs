//! Formula syntax: parsing, printing, and reference rewriting.
//!
//! Precedence, loosest to tightest: comparisons, `&`, `+ -`, `* /`, `^`,
//! unary `- +`, postfix `%` — so `-2^2` is 4 and `2^3^2` is 64, as in Excel.

use crate::refs::{CellRef, Key, MAX_COLS, MAX_ROWS};
use crate::value::ErrorKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pow => "^",
            BinOp::Concat => "&",
            BinOp::Eq => "=",
            BinOp::Ne => "<>",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Text(String),
    Bool(bool),
    Ref(CellRef),
    Range(CellRef, CellRef),
    Neg(Box<Expr>),
    Percent(Box<Expr>),
    /// Kept so a formula prints back the way it was typed.
    Paren(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Err(ErrorKind),
}

/// What a formula reads: single cells and rectangular ranges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dep {
    Cell(Key),
    Range(Key, Key),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Row,
    Col,
}

/// Rewrites both ends of a range together (`None` makes it `#REF!`).
type RangeMap<'a> = dyn Fn(&CellRef, &CellRef) -> Option<(CellRef, CellRef)> + 'a;

// ---- tokens --------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Err(ErrorKind),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
    Colon,
    Percent,
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    let mut j = i + 1;
                    if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                        j += 1;
                    }
                    if j < chars.len() && chars[j].is_ascii_digit() {
                        while j < chars.len() && chars[j].is_ascii_digit() {
                            j += 1;
                        }
                        i = j;
                    }
                }
                let text: String = chars[start..i].iter().collect();
                out.push(Tok::Num(
                    text.parse().map_err(|_| format!("bad number {text}"))?,
                ));
            }
            '"' => {
                let mut s = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err("unterminated text".into()),
                        Some('"') if chars.get(i + 1) == Some(&'"') => {
                            s.push('"');
                            i += 2;
                        }
                        Some('"') => {
                            i += 1;
                            break;
                        }
                        Some(&c) => {
                            s.push(c);
                            i += 1;
                        }
                    }
                }
                out.push(Tok::Str(s));
            }
            '#' => {
                let start = i;
                i += 1;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || "/!?".contains(chars[i]))
                {
                    i += 1;
                    if matches!(chars[i - 1], '!' | '?') {
                        break;
                    }
                }
                let text: String = chars[start..i].iter().collect();
                out.push(Tok::Err(
                    ErrorKind::from_code(&text).ok_or(format!("unknown error {text}"))?,
                ));
            }
            c if c.is_alphabetic() || c == '_' || c == '$' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '.' | '$'))
                {
                    i += 1;
                }
                if chars.get(i) == Some(&'!') {
                    return Err("references to other sheets are not supported yet".into());
                }
                out.push(Tok::Ident(chars[start..i].iter().collect()));
            }
            '<' | '>' => {
                let two = chars.get(i + 1).copied();
                let op = match (c, two) {
                    ('<', Some('=')) => "<=",
                    ('<', Some('>')) => "<>",
                    ('>', Some('=')) => ">=",
                    ('<', _) => "<",
                    _ => ">",
                };
                i += op.len();
                out.push(Tok::Op(op));
            }
            '+' | '-' | '*' | '/' | '^' | '&' | '=' => {
                out.push(Tok::Op(match c {
                    '+' => "+",
                    '-' => "-",
                    '*' => "*",
                    '/' => "/",
                    '^' => "^",
                    '&' => "&",
                    _ => "=",
                }));
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            ':' => {
                out.push(Tok::Colon);
                i += 1;
            }
            '%' => {
                out.push(Tok::Percent);
                i += 1;
            }
            other => return Err(format!("unexpected character '{other}'")),
        }
    }
    Ok(out)
}

// ---- parser --------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn binary(&mut self, level: u8) -> Result<Expr, String> {
        // Levels: 0 comparison, 1 concat, 2 additive, 3 multiplicative, 4 power.
        if level == 5 {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        loop {
            let op = match (level, self.peek()) {
                (0, Some(Tok::Op("="))) => BinOp::Eq,
                (0, Some(Tok::Op("<>"))) => BinOp::Ne,
                (0, Some(Tok::Op("<"))) => BinOp::Lt,
                (0, Some(Tok::Op("<="))) => BinOp::Le,
                (0, Some(Tok::Op(">"))) => BinOp::Gt,
                (0, Some(Tok::Op(">="))) => BinOp::Ge,
                (1, Some(Tok::Op("&"))) => BinOp::Concat,
                (2, Some(Tok::Op("+"))) => BinOp::Add,
                (2, Some(Tok::Op("-"))) => BinOp::Sub,
                (3, Some(Tok::Op("*"))) => BinOp::Mul,
                (3, Some(Tok::Op("/"))) => BinOp::Div,
                (4, Some(Tok::Op("^"))) => BinOp::Pow,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.binary(level + 1)?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some(Tok::Op("-")) => {
                self.pos += 1;
                Ok(Expr::Neg(Box::new(self.unary()?)))
            }
            Some(Tok::Op("+")) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.primary()?;
        while self.eat(&Tok::Percent) {
            e = Expr::Percent(Box::new(e));
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Num(n)) => {
                // "1:3" is a range of whole rows.
                if self.peek() == Some(&Tok::Colon) {
                    if let Some(Tok::Num(m)) = self.toks.get(self.pos + 1).cloned() {
                        self.pos += 2;
                        return row_range(n, m);
                    }
                }
                Ok(Expr::Num(n))
            }
            Some(Tok::Str(s)) => Ok(Expr::Text(s)),
            Some(Tok::Err(e)) => Ok(Expr::Err(e)),
            Some(Tok::LParen) => {
                let inner = self.binary(0)?;
                if !self.eat(&Tok::RParen) {
                    return Err("missing )".into());
                }
                Ok(Expr::Paren(Box::new(inner)))
            }
            Some(Tok::Ident(name)) => self.ident(name),
            Some(t) => Err(format!("unexpected {t:?}")),
            None => Err("formula ends too soon".into()),
        }
    }

    fn ident(&mut self, name: String) -> Result<Expr, String> {
        if self.eat(&Tok::LParen) {
            let mut args = Vec::new();
            if !self.eat(&Tok::RParen) {
                loop {
                    args.push(self.binary(0)?);
                    if self.eat(&Tok::RParen) {
                        break;
                    }
                    if !self.eat(&Tok::Comma) {
                        return Err("expected , or )".into());
                    }
                }
            }
            return Ok(Expr::Call(name.to_ascii_uppercase(), args));
        }
        if name.eq_ignore_ascii_case("TRUE") {
            return Ok(Expr::Bool(true));
        }
        if name.eq_ignore_ascii_case("FALSE") {
            return Ok(Expr::Bool(false));
        }
        if let Some(a) = CellRef::parse(&name) {
            if self.peek() == Some(&Tok::Colon) {
                if let Some(Tok::Ident(other)) = self.toks.get(self.pos + 1).cloned() {
                    if let Some(b) = CellRef::parse(&other) {
                        self.pos += 2;
                        return Ok(Expr::Range(a, b));
                    }
                }
            }
            return Ok(Expr::Ref(a));
        }
        // "A:C" is a range of whole columns.
        if self.peek() == Some(&Tok::Colon) {
            if let Some(Tok::Ident(other)) = self.toks.get(self.pos + 1).cloned() {
                if let (Some(a), Some(b)) = (whole_col(&name), whole_col(&other)) {
                    self.pos += 2;
                    return Ok(Expr::Range(a.0, b.1));
                }
            }
        }
        // Unknown names are a runtime #NAME?, not a syntax error.
        Ok(Expr::Err(ErrorKind::Name))
    }
}

fn row_range(a: f64, b: f64) -> Result<Expr, String> {
    let ok = |n: f64| n.fract() == 0.0 && n >= 1.0 && n <= MAX_ROWS as f64;
    if !ok(a) || !ok(b) {
        return Err("bad row range".into());
    }
    let mk = |r: f64, c: u32| CellRef::new(r as u32 - 1, c);
    Ok(Expr::Range(mk(a, 0), mk(b, MAX_COLS - 1)))
}

/// Column letters as the first and last cells of that whole column.
fn whole_col(s: &str) -> Option<(CellRef, CellRef)> {
    let (abs, letters) = match s.strip_prefix('$') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let col = crate::refs::parse_col(letters)?;
    let cell = |row| CellRef {
        row,
        col,
        abs_row: false,
        abs_col: abs,
    };
    Some((cell(0), cell(MAX_ROWS - 1)))
}

/// Parse the text after the leading `=`.
pub fn parse(src: &str) -> Result<Expr, String> {
    let toks = tokenize(src)?;
    if toks.is_empty() {
        return Err("empty formula".into());
    }
    let mut p = Parser { toks, pos: 0 };
    let e = p.binary(0)?;
    if p.pos < p.toks.len() {
        return Err(format!("unexpected {:?}", p.toks[p.pos]));
    }
    Ok(e)
}

// ---- printing ------------------------------------------------------------

fn print_ref(r: &CellRef) -> String {
    r.to_a1()
}

impl Expr {
    /// The formula text (without the leading `=`).
    pub fn to_formula(&self) -> String {
        match self {
            Expr::Num(n) => format!("{n}"),
            Expr::Text(s) => format!("\"{}\"", s.replace('"', "\"\"")),
            Expr::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
            Expr::Ref(r) => print_ref(r),
            Expr::Range(a, b) => {
                if a.row == 0 && b.row == MAX_ROWS - 1 && !a.abs_row && !b.abs_row {
                    format!(
                        "{}:{}",
                        print_ref(a).trim_end_matches('1'),
                        print_ref(b).trim_end_matches("1048576")
                    )
                } else if a.col == 0 && b.col == MAX_COLS - 1 && !a.abs_col && !b.abs_col {
                    format!("{}:{}", a.row + 1, b.row + 1)
                } else {
                    format!("{}:{}", print_ref(a), print_ref(b))
                }
            }
            Expr::Neg(e) => format!("-{}", e.to_formula()),
            Expr::Percent(e) => format!("{}%", e.to_formula()),
            Expr::Paren(e) => format!("({})", e.to_formula()),
            Expr::Bin(op, l, r) => format!("{}{}{}", l.to_formula(), op.symbol(), r.to_formula()),
            Expr::Call(name, args) => {
                format!(
                    "{name}({})",
                    args.iter()
                        .map(Expr::to_formula)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
            Expr::Err(e) => e.code().to_string(),
        }
    }

    /// Rebuild with every reference rewritten by `f`; a `None` result turns
    /// that reference into `#REF!`.
    fn map_refs(&self, f: &dyn Fn(&CellRef) -> Option<CellRef>, range: &RangeMap) -> Expr {
        let m = |e: &Expr| Box::new(e.map_refs(f, range));
        match self {
            Expr::Ref(r) => f(r).map_or(Expr::Err(ErrorKind::Ref), Expr::Ref),
            Expr::Range(a, b) => {
                range(a, b).map_or(Expr::Err(ErrorKind::Ref), |(a, b)| Expr::Range(a, b))
            }
            Expr::Neg(e) => Expr::Neg(m(e)),
            Expr::Percent(e) => Expr::Percent(m(e)),
            Expr::Paren(e) => Expr::Paren(m(e)),
            Expr::Bin(op, l, r) => Expr::Bin(*op, m(l), m(r)),
            Expr::Call(n, args) => Expr::Call(
                n.clone(),
                args.iter().map(|a| a.map_refs(f, range)).collect(),
            ),
            other => other.clone(),
        }
    }

    /// The formula as if copied `(dr, dc)` cells away (relative parts move).
    pub fn shifted(&self, dr: i64, dc: i64) -> Expr {
        self.map_refs(&|r| r.shifted(dr, dc), &|a, b| {
            Some((a.shifted(dr, dc)?, b.shifted(dr, dc)?))
        })
    }

    /// Adjust for `count` rows/columns inserted before index `at`.
    pub fn after_insert(&self, axis: Axis, at: u32, count: u32) -> Expr {
        let bump = move |v: u32| if v >= at { v + count } else { v };
        let one = move |r: &CellRef| {
            let mut n = *r;
            match axis {
                Axis::Row => n.row = bump(r.row),
                Axis::Col => n.col = bump(r.col),
            }
            (n.row < MAX_ROWS && n.col < MAX_COLS).then_some(n)
        };
        self.map_refs(&one, &move |a, b| Some((one(a)?, one(b)?)))
    }

    /// Adjust for `count` rows/columns deleted starting at index `at`.
    /// References to deleted cells become `#REF!`; ranges shrink.
    pub fn after_delete(&self, axis: Axis, at: u32, count: u32) -> Expr {
        let end = at + count; // first surviving index after the deleted block
        let coord = move |r: &CellRef| match axis {
            Axis::Row => r.row,
            Axis::Col => r.col,
        };
        let with = move |r: &CellRef, v: u32| {
            let mut n = *r;
            match axis {
                Axis::Row => n.row = v,
                Axis::Col => n.col = v,
            }
            n
        };
        let one = move |r: &CellRef| {
            let v = coord(r);
            if v < at {
                Some(*r)
            } else if v >= end {
                Some(with(r, v - count))
            } else {
                None
            }
        };
        let rng = move |a: &CellRef, b: &CellRef| {
            let (s, e) = (coord(a), coord(b));
            if s >= at && e < end {
                return None; // the whole range is gone
            }
            let ns = if s < at {
                s
            } else if s >= end {
                s - count
            } else {
                at
            };
            let ne = if e < at {
                e
            } else if e >= end {
                e - count
            } else {
                at - 1
            };
            Some((with(a, ns), with(b, ne)))
        };
        self.map_refs(&one, &rng)
    }

    /// Every cell and range this expression reads.
    pub fn deps(&self, out: &mut Vec<Dep>) {
        match self {
            Expr::Ref(r) => out.push(Dep::Cell(r.key())),
            Expr::Range(a, b) => {
                let (r1, r2) = (a.row.min(b.row), a.row.max(b.row));
                let (c1, c2) = (a.col.min(b.col), a.col.max(b.col));
                out.push(Dep::Range((r1, c1), (r2, c2)));
            }
            Expr::Neg(e) | Expr::Percent(e) | Expr::Paren(e) => e.deps(out),
            Expr::Bin(_, l, r) => {
                l.deps(out);
                r.deps(out);
            }
            Expr::Call(_, args) => args.iter().for_each(|a| a.deps(out)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Expr {
        parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn round_trip(s: &str) {
        assert_eq!(p(s).to_formula(), s, "printing {s}");
    }

    #[test]
    fn precedence_matches_excel() {
        let n = |v| Box::new(Expr::Num(v));
        assert_eq!(
            p("1+2*3"),
            Expr::Bin(
                BinOp::Add,
                n(1.0),
                Box::new(Expr::Bin(BinOp::Mul, n(2.0), n(3.0)))
            )
        );
        // -2^2 → (-2)^2
        assert_eq!(
            p("-2^2"),
            Expr::Bin(BinOp::Pow, Box::new(Expr::Neg(n(2.0))), n(2.0))
        );
        // 2^3^2 → (2^3)^2 (left associative)
        assert_eq!(
            p("2^3^2"),
            Expr::Bin(
                BinOp::Pow,
                Box::new(Expr::Bin(BinOp::Pow, n(2.0), n(3.0))),
                n(2.0)
            )
        );
        // 50%*2 and percent binds tighter than ^
        assert_eq!(
            p("50%*2"),
            Expr::Bin(BinOp::Mul, Box::new(Expr::Percent(n(50.0))), n(2.0))
        );
        // a&b=c → (a&b)=c ; 1+2&3 → (1+2)&3
        assert!(matches!(p("1&2=3"), Expr::Bin(BinOp::Eq, ..)));
        assert!(matches!(p("1+2&3"), Expr::Bin(BinOp::Concat, ..)));
        assert_eq!(p("+5"), Expr::Num(5.0));
    }

    #[test]
    fn literals_references_and_ranges() {
        assert_eq!(p("TRUE"), Expr::Bool(true));
        assert_eq!(p("false"), Expr::Bool(false));
        assert_eq!(p("\"a\"\"b\""), Expr::Text("a\"b".into()));
        assert_eq!(p("1.5e3"), Expr::Num(1500.0));
        assert_eq!(p(".5"), Expr::Num(0.5));
        assert_eq!(p("#N/A"), Expr::Err(ErrorKind::Na));
        assert_eq!(p("#DIV/0!"), Expr::Err(ErrorKind::DivZero));
        assert!(matches!(p("$A$1"), Expr::Ref(r) if r.abs_row && r.abs_col));
        assert!(matches!(p("A1:B3"), Expr::Range(a, b) if a.key() == (0, 0) && b.key() == (2, 1)));
        assert!(
            matches!(p("A:C"), Expr::Range(a, b) if a.key() == (0, 0) && b.key() == (MAX_ROWS - 1, 2))
        );
        assert!(
            matches!(p("2:4"), Expr::Range(a, b) if a.key() == (1, 0) && b.key() == (3, MAX_COLS - 1))
        );
        assert_eq!(p("nosuchname"), Expr::Err(ErrorKind::Name));
        assert_eq!(p("sum(a1:a3)"), Expr::Call("SUM".into(), vec![p("A1:A3")]));
        assert_eq!(p("PI()"), Expr::Call("PI".into(), vec![]));
        assert_eq!(p("  1 +  2 "), p("1+2"), "whitespace is ignored");
    }

    #[test]
    fn syntax_errors_are_reported_not_panics() {
        for bad in [
            "",
            "1+",
            "(1",
            "1)",
            "1 2",
            "SUM(1,",
            "SUM(1 2)",
            "\"abc",
            "1+*2",
            "@",
            "A1:",
            "Sheet2!A1",
            "#BOGUS",
            "1:",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} should not parse");
        }
        assert!(parse("1:0").is_err(), "row 0 does not exist");
    }

    #[test]
    fn printing_round_trips_typed_formulas() {
        for f in [
            "1+2*3",
            "(1+2)*3",
            "-A1+$B$2",
            "SUM(A1:B3,5,\"x\")",
            "IF(A1>=10,\"big\",\"small\")",
            "A1&\" \"&B1",
            "50%+1",
            "A:C",
            "2:4",
            "$A:$A",
            "2^3^2",
            "#REF!+1",
            "AVERAGE(A1:A9)/COUNT(A1:A9)",
            "1.5",
            "\"say \"\"hi\"\"\"",
        ] {
            round_trip(f);
        }
        assert_eq!(
            p("sum( a1 , a2 )").to_formula(),
            "SUM(A1,A2)",
            "names are normalised"
        );
    }

    #[test]
    fn copying_shifts_relative_parts_only() {
        let f = p("A1+$B$2+C$3+$D4+SUM(A1:B2)");
        assert_eq!(f.shifted(1, 1).to_formula(), "B2+$B$2+D$3+$D5+SUM(B2:C3)");
        assert_eq!(f.shifted(0, 0), f);
        // Moving off the sheet gives #REF!, only for the references that fall off.
        assert_eq!(p("A1+B1").shifted(0, -1).to_formula(), "#REF!+A1");
        assert_eq!(p("SUM(A1:B2)").shifted(-1, 0).to_formula(), "SUM(#REF!)");
    }

    #[test]
    fn inserting_moves_references_at_or_after_the_new_rows() {
        let f = p("A1+A2+A3+$A$4+SUM(A1:A3)");
        // Insert 2 rows before row 2 (index 1).
        assert_eq!(
            f.after_insert(Axis::Row, 1, 2).to_formula(),
            "A1+A4+A5+$A$6+SUM(A1:A5)"
        );
        // Columns.
        assert_eq!(
            p("A1+B1+SUM(A1:B1)")
                .after_insert(Axis::Col, 1, 1)
                .to_formula(),
            "A1+C1+SUM(A1:C1)"
        );
        // Inserting just after a range doesn't grow it; at its first row shifts it.
        assert_eq!(
            p("SUM(A1:A3)").after_insert(Axis::Row, 3, 1).to_formula(),
            "SUM(A1:A3)"
        );
        assert_eq!(
            p("SUM(A2:A3)").after_insert(Axis::Row, 1, 1).to_formula(),
            "SUM(A3:A4)"
        );
    }

    #[test]
    fn deleting_breaks_references_to_deleted_cells_and_shrinks_ranges() {
        let f = p("A1+A2+A3+A4");
        assert_eq!(
            f.after_delete(Axis::Row, 1, 2).to_formula(),
            "A1+#REF!+#REF!+A2"
        );
        assert_eq!(
            p("SUM(A1:A5)").after_delete(Axis::Row, 1, 2).to_formula(),
            "SUM(A1:A3)"
        );
        assert_eq!(
            p("SUM(A2:A3)").after_delete(Axis::Row, 1, 2).to_formula(),
            "SUM(#REF!)",
            "whole range deleted"
        );
        assert_eq!(
            p("SUM(A1:A3)").after_delete(Axis::Row, 2, 5).to_formula(),
            "SUM(A1:A2)",
            "tail deleted"
        );
        assert_eq!(
            p("SUM(A3:A6)").after_delete(Axis::Row, 0, 3).to_formula(),
            "SUM(A1:A3)",
            "head deleted"
        );
        assert_eq!(
            p("SUM(A1:C1)").after_delete(Axis::Col, 1, 1).to_formula(),
            "SUM(A1:B1)"
        );
        assert_eq!(p("C1").after_delete(Axis::Col, 0, 1).to_formula(), "B1");
    }

    #[test]
    fn dependencies_are_collected() {
        let mut d = Vec::new();
        p("A1+SUM(B2:C3)*IF(D4,E5,1)").deps(&mut d);
        assert_eq!(
            d,
            vec![
                Dep::Cell((0, 0)),
                Dep::Range((1, 1), (2, 2)),
                Dep::Cell((3, 3)),
                Dep::Cell((4, 4))
            ]
        );
        let mut d = Vec::new();
        p("SUM(C3:B2)").deps(&mut d);
        assert_eq!(
            d,
            vec![Dep::Range((1, 1), (2, 2))],
            "reversed ranges are normalised"
        );
    }
}
