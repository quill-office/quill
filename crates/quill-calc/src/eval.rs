//! Evaluating formulas: expressions, coercion, and the function library.

use std::cmp::Ordering;

use crate::expr::{BinOp, Expr};
use crate::refs::Key;
use crate::sheet::{parse_number, Sheet};
use crate::value::{round_half_away, ErrorKind, Value};

type R<T> = Result<T, ErrorKind>;

/// A function argument: a plain value, a value that came from a single cell
/// reference (so text is ignored by `SUM`), or a rectangle of cells.
enum Arg {
    Scalar(Value),
    Cell(Value),
    /// Normalised: `(top, left)`, `(bottom, right)`.
    Range(Key, Key),
}

fn to_num(v: &Value) -> R<f64> {
    match v {
        Value::Empty => Ok(0.0),
        Value::Number(n) => Ok(*n),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Text(s) => parse_number(s).ok_or(ErrorKind::Value),
        Value::Error(e) => Err(*e),
    }
}

fn to_bool(v: &Value) -> R<bool> {
    match v {
        Value::Empty => Ok(false),
        Value::Number(n) => Ok(*n != 0.0),
        Value::Bool(b) => Ok(*b),
        Value::Text(s) if s.eq_ignore_ascii_case("true") => Ok(true),
        Value::Text(s) if s.eq_ignore_ascii_case("false") => Ok(false),
        Value::Text(_) => Err(ErrorKind::Value),
        Value::Error(e) => Err(*e),
    }
}

fn to_text(v: &Value) -> R<String> {
    match v {
        Value::Error(e) => Err(*e),
        other => Ok(other.to_text()),
    }
}

/// A finite number, or `#NUM!`.
fn num(n: f64) -> Value {
    if n.is_finite() {
        Value::Number(n)
    } else {
        Value::Error(ErrorKind::Num)
    }
}

fn rank(v: &Value) -> u8 {
    match v {
        Value::Number(_) => 0,
        Value::Text(_) => 1,
        _ => 2,
    }
}

/// Excel's ordering: numbers < text < logicals; text ignores case; blanks
/// compare as 0, "" or FALSE depending on the other side.
fn compare(a: &Value, b: &Value) -> Ordering {
    let blank_as = |other: &Value| match other {
        Value::Text(_) => Value::Text(String::new()),
        Value::Bool(_) => Value::Bool(false),
        _ => Value::Number(0.0),
    };
    let (a, b) = match (a, b) {
        (Value::Empty, Value::Empty) => return Ordering::Equal,
        (Value::Empty, o) => (blank_as(o), o.clone()),
        (o, Value::Empty) => (o.clone(), blank_as(o)),
        (x, y) => (x.clone(), y.clone()),
    };
    match (&a, &b) {
        (Value::Number(x), Value::Number(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Text(x), Value::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        _ => rank(&a).cmp(&rank(&b)),
    }
}

/// Case-insensitive match with `*` (any run) and `?` (any one char).
fn wildcard(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.to_lowercase().chars().collect();
    let t: Vec<char> = s.to_lowercase().chars().collect();
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < t.len() {
        if i < p.len() && (p[i] == '?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(si) = star {
            i = si + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}

/// A `SUMIF`/`COUNTIF` condition such as `">=10"`, `"apple*"` or `5`.
struct Criteria {
    op: Ordering,
    /// `<>` is "not equal"; `<=`/`>=` combine an ordering with equality.
    or_equal: bool,
    negate: bool,
    operand: Value,
}

fn infer(s: &str) -> Value {
    if let Some(n) = parse_number(s) {
        Value::Number(n)
    } else if s.eq_ignore_ascii_case("true") {
        Value::Bool(true)
    } else if s.eq_ignore_ascii_case("false") {
        Value::Bool(false)
    } else {
        Value::Text(s.to_string())
    }
}

fn criteria(v: &Value) -> Criteria {
    let Value::Text(s) = v else {
        return Criteria {
            op: Ordering::Equal,
            or_equal: true,
            negate: false,
            operand: v.clone(),
        };
    };
    for (prefix, op, or_equal, negate) in [
        ("<=", Ordering::Less, true, false),
        (">=", Ordering::Greater, true, false),
        ("<>", Ordering::Equal, true, true),
        ("<", Ordering::Less, false, false),
        (">", Ordering::Greater, false, false),
        ("=", Ordering::Equal, true, false),
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return Criteria {
                op,
                or_equal,
                negate,
                operand: infer(rest),
            };
        }
    }
    Criteria {
        op: Ordering::Equal,
        or_equal: true,
        negate: false,
        operand: infer(s),
    }
}

impl Criteria {
    fn matches(&self, v: &Value) -> bool {
        let hit = match (&self.operand, v) {
            (_, Value::Error(_)) => false,
            (Value::Number(_), Value::Number(_)) | (Value::Bool(_), Value::Bool(_)) => {
                let o = compare(v, &self.operand);
                o == self.op || (self.or_equal && o == Ordering::Equal)
            }
            (Value::Text(pat), Value::Text(s)) if self.op == Ordering::Equal => wildcard(pat, s),
            (Value::Text(pat), Value::Text(s)) => {
                let o = s.to_lowercase().cmp(&pat.to_lowercase());
                o == self.op || (self.or_equal && o == Ordering::Equal)
            }
            _ => false,
        };
        // "<>" matches everything that isn't equal, including other types and blanks.
        if self.negate {
            !hit
        } else {
            hit
        }
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    !matches!(b, Value::Empty | Value::Error(_))
        && rank(a) == rank(b)
        && compare(a, b) == Ordering::Equal
}

impl Sheet {
    // ---- cells ------------------------------------------------------------

    /// Evaluate a formula cell and store its value.
    pub(crate) fn compute(&mut self, key: Key) -> Value {
        if !self.evaluating.insert(key) {
            return Value::Error(ErrorKind::Circular);
        }
        let expr = self.cells.get(&key).and_then(|c| c.formula.clone());
        let value = match expr {
            Some(e) => match self.eval(&e) {
                Value::Empty => Value::Number(0.0), // =A1 on an empty cell shows 0
                Value::Number(n) if !n.is_finite() => Value::Error(ErrorKind::Num),
                v => v,
            },
            None => self.value(key),
        };
        self.evaluating.remove(&key);
        if let Some(c) = self.cells.get_mut(&key) {
            c.value = value.clone();
        }
        self.stale.remove(&key);
        value
    }

    fn value_at(&mut self, key: Key) -> Value {
        match self.cells.get(&key) {
            None => Value::Empty,
            Some(c) if c.formula.is_some() && self.stale.contains(&key) => self.compute(key),
            Some(c) => c.value.clone(),
        }
    }

    /// Existing, non-empty cells of a rectangle with their (fresh) values.
    fn range_cells(&mut self, a: Key, b: Key) -> Vec<(Key, Value)> {
        self.keys_in(a, b)
            .into_iter()
            .map(|k| (k, self.value_at(k)))
            .filter(|(_, v)| *v != Value::Empty)
            .collect()
    }

    /// A rectangle as rows of values, trimmed to the used area so whole
    /// rows/columns stay cheap.
    fn matrix(&mut self, a: Key, b: Key) -> Vec<Vec<Value>> {
        let last_row = self.cells.keys().next_back().map_or(0, |k| k.0);
        let r2 = b.0.min(last_row);
        let c2 = b.1.min(self.max_col);
        if a.0 > r2 || a.1 > c2 {
            return vec![];
        }
        (a.0..=r2)
            .map(|r| (a.1..=c2).map(|c| self.value_at((r, c))).collect())
            .collect()
    }

    // ---- expressions ------------------------------------------------------

    fn arg(&mut self, e: &Expr) -> Arg {
        match e {
            Expr::Ref(r) => Arg::Cell(self.value_at(r.key())),
            Expr::Range(a, b) => Arg::Range(
                (a.row.min(b.row), a.col.min(b.col)),
                (a.row.max(b.row), a.col.max(b.col)),
            ),
            Expr::Paren(inner) => self.arg(inner),
            other => Arg::Scalar(self.eval(other)),
        }
    }

    pub(crate) fn eval(&mut self, e: &Expr) -> Value {
        match e {
            Expr::Num(n) => Value::Number(*n),
            Expr::Text(s) => Value::Text(s.clone()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Err(k) => Value::Error(*k),
            Expr::Ref(r) => self.value_at(r.key()),
            // A range where a single value is expected.
            Expr::Range(a, b) if a.key() == b.key() => self.value_at(a.key()),
            Expr::Range(..) => Value::Error(ErrorKind::Value),
            Expr::Paren(inner) => self.eval(inner),
            Expr::Neg(inner) => match self.eval(inner) {
                Value::Error(k) => Value::Error(k),
                v => to_num(&v).map_or_else(Value::Error, |n| Value::Number(-n)),
            },
            Expr::Percent(inner) => match self.eval(inner) {
                Value::Error(k) => Value::Error(k),
                v => to_num(&v).map_or_else(Value::Error, |n| Value::Number(n / 100.0)),
            },
            Expr::Bin(op, l, r) => {
                let (a, b) = (self.eval(l), self.eval(r));
                self.binary(*op, a, b).unwrap_or_else(Value::Error)
            }
            Expr::Call(name, args) => self.call(name, args).unwrap_or_else(Value::Error),
        }
    }

    fn binary(&mut self, op: BinOp, a: Value, b: Value) -> R<Value> {
        for v in [&a, &b] {
            if let Value::Error(e) = v {
                return Err(*e);
            }
        }
        Ok(match op {
            BinOp::Concat => Value::Text(a.to_text() + &b.to_text()),
            BinOp::Add => num(to_num(&a)? + to_num(&b)?),
            BinOp::Sub => num(to_num(&a)? - to_num(&b)?),
            BinOp::Mul => num(to_num(&a)? * to_num(&b)?),
            BinOp::Div => {
                let d = to_num(&b)?;
                if d == 0.0 {
                    return Err(ErrorKind::DivZero);
                }
                num(to_num(&a)? / d)
            }
            BinOp::Pow => {
                let (x, y) = (to_num(&a)?, to_num(&b)?);
                if x == 0.0 && y == 0.0 {
                    return Err(ErrorKind::Num);
                }
                if x == 0.0 && y < 0.0 {
                    return Err(ErrorKind::DivZero);
                }
                num(x.powf(y))
            }
            cmp => {
                let o = compare(&a, &b);
                Value::Bool(match cmp {
                    BinOp::Eq => o == Ordering::Equal,
                    BinOp::Ne => o != Ordering::Equal,
                    BinOp::Lt => o == Ordering::Less,
                    BinOp::Le => o != Ordering::Greater,
                    BinOp::Gt => o == Ordering::Greater,
                    _ => o != Ordering::Less,
                })
            }
        })
    }

    // ---- argument helpers -------------------------------------------------

    fn scalar(&mut self, e: &Expr) -> R<Value> {
        match self.arg(e) {
            Arg::Scalar(v) | Arg::Cell(v) => match v {
                Value::Error(k) => Err(k),
                v => Ok(v),
            },
            Arg::Range(a, b) if a == b => Ok(self.value_at(a)),
            Arg::Range(..) => Err(ErrorKind::Value),
        }
    }

    fn number_arg(&mut self, e: &Expr) -> R<f64> {
        to_num(&self.scalar(e)?)
    }

    fn text_arg(&mut self, e: &Expr) -> R<String> {
        to_text(&self.scalar(e)?)
    }

    /// Numbers from a mix of values and ranges, the way `SUM` reads them:
    /// numbers inside ranges/references only; directly typed values coerce.
    fn numbers(&mut self, args: &[Expr]) -> R<Vec<f64>> {
        let mut out = Vec::new();
        for a in args {
            match self.arg(a) {
                Arg::Scalar(v) => out.push(to_num(&v)?),
                Arg::Cell(Value::Number(n)) => out.push(n),
                Arg::Cell(Value::Error(e)) => return Err(e),
                Arg::Cell(_) => {}
                Arg::Range(p, q) => {
                    for (_, v) in self.range_cells(p, q) {
                        match v {
                            Value::Number(n) => out.push(n),
                            Value::Error(e) => return Err(e),
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    /// Every value an argument list denotes, ranges flattened (blanks skipped).
    fn flatten(&mut self, args: &[Expr]) -> Vec<Value> {
        let mut out = Vec::new();
        for a in args {
            match self.arg(a) {
                Arg::Scalar(v) | Arg::Cell(v) => out.push(v),
                Arg::Range(p, q) => out.extend(self.range_cells(p, q).into_iter().map(|(_, v)| v)),
            }
        }
        out
    }

    fn range_arg(&mut self, e: &Expr) -> R<(Key, Key)> {
        match self.arg(e) {
            Arg::Range(a, b) => Ok((a, b)),
            Arg::Cell(_) | Arg::Scalar(_) => match e {
                Expr::Ref(r) => Ok((r.key(), r.key())),
                _ => Err(ErrorKind::Value),
            },
        }
    }

    fn need(args: &[Expr], min: usize, max: usize) -> R<()> {
        if (min..=max).contains(&args.len()) {
            Ok(())
        } else {
            Err(ErrorKind::Value)
        }
    }

    fn int_arg(&mut self, e: &Expr) -> R<i64> {
        Ok(self.number_arg(e)?.trunc() as i64)
    }

    // ---- functions ----------------------------------------------------------

    fn call(&mut self, name: &str, args: &[Expr]) -> R<Value> {
        let n = Self::need;
        Ok(match name {
            // Aggregates
            "SUM" => num(self.numbers(args)?.iter().sum()),
            "PRODUCT" => num(self.numbers(args)?.iter().product()),
            "AVERAGE" => {
                let v = self.numbers(args)?;
                if v.is_empty() {
                    return Err(ErrorKind::DivZero);
                }
                num(v.iter().sum::<f64>() / v.len() as f64)
            }
            "MIN" => Value::Number(
                self.numbers(args)?
                    .into_iter()
                    .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.min(x))))
                    .unwrap_or(0.0),
            ),
            "MAX" => Value::Number(
                self.numbers(args)?
                    .into_iter()
                    .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))))
                    .unwrap_or(0.0),
            ),
            "MEDIAN" => {
                let mut v = self.numbers(args)?;
                if v.is_empty() {
                    return Err(ErrorKind::Num);
                }
                v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
                let m = v.len() / 2;
                Value::Number(if v.len() % 2 == 1 {
                    v[m]
                } else {
                    (v[m - 1] + v[m]) / 2.0
                })
            }
            "COUNT" => {
                let mut c = 0;
                for a in args {
                    match self.arg(a) {
                        Arg::Scalar(v) => c += usize::from(v != Value::Empty && to_num(&v).is_ok()),
                        Arg::Cell(v) => c += usize::from(matches!(v, Value::Number(_))),
                        Arg::Range(p, q) => {
                            c += self
                                .range_cells(p, q)
                                .iter()
                                .filter(|(_, v)| matches!(v, Value::Number(_)))
                                .count()
                        }
                    }
                }
                Value::Number(c as f64)
            }
            "COUNTA" => Value::Number(
                self.flatten(args)
                    .iter()
                    .filter(|v| **v != Value::Empty)
                    .count() as f64,
            ),
            "COUNTBLANK" => {
                n(args, 1, 1)?;
                let (a, b) = self.range_arg(&args[0])?;
                let area = (b.0 - a.0 + 1) as f64 * (b.1 - a.1 + 1) as f64;
                Value::Number(area - self.range_cells(a, b).len() as f64)
            }

            // Math
            "ABS" => {
                n(args, 1, 1)?;
                Value::Number(self.number_arg(&args[0])?.abs())
            }
            "SIGN" => {
                n(args, 1, 1)?;
                let x = self.number_arg(&args[0])?;
                Value::Number(if x > 0.0 {
                    1.0
                } else if x < 0.0 {
                    -1.0
                } else {
                    0.0
                })
            }
            "INT" => {
                n(args, 1, 1)?;
                Value::Number(self.number_arg(&args[0])?.floor())
            }
            "SQRT" => {
                n(args, 1, 1)?;
                let x = self.number_arg(&args[0])?;
                if x < 0.0 {
                    return Err(ErrorKind::Num);
                }
                Value::Number(x.sqrt())
            }
            "EXP" => {
                n(args, 1, 1)?;
                num(self.number_arg(&args[0])?.exp())
            }
            "LN" | "LOG10" | "LOG" => {
                n(args, 1, if name == "LOG" { 2 } else { 1 })?;
                let x = self.number_arg(&args[0])?;
                let base = if name == "LOG" && args.len() == 2 {
                    self.number_arg(&args[1])?
                } else {
                    10.0
                };
                if x <= 0.0 || (name == "LOG" && (base <= 0.0 || base == 1.0)) {
                    return Err(ErrorKind::Num);
                }
                Value::Number(match name {
                    "LN" => x.ln(),
                    "LOG10" => x.log10(),
                    _ => x.ln() / base.ln(),
                })
            }
            "PI" => {
                n(args, 0, 0)?;
                Value::Number(std::f64::consts::PI)
            }
            "POWER" => {
                n(args, 2, 2)?;
                let (a, b) = (self.scalar(&args[0])?, self.scalar(&args[1])?);
                return self.binary(BinOp::Pow, a, b);
            }
            "MOD" => {
                n(args, 2, 2)?;
                let (x, d) = (self.number_arg(&args[0])?, self.number_arg(&args[1])?);
                if d == 0.0 {
                    return Err(ErrorKind::DivZero);
                }
                num(x - d * (x / d).floor())
            }
            "ROUND" | "ROUNDUP" | "ROUNDDOWN" => {
                n(args, 1, 2)?;
                let x = self.number_arg(&args[0])?;
                let d = if args.len() == 2 {
                    self.int_arg(&args[1])? as i32
                } else {
                    0
                };
                let p = 10f64.powi(d.clamp(-300, 300));
                Value::Number(match name {
                    "ROUND" => round_half_away(x, d),
                    // Scaling noise (1.15 * 100 = 114.99999999999999) must not drop a digit.
                    "ROUNDUP" => x.signum() * (x.abs() * p * (1.0 - f64::EPSILON)).ceil() / p,
                    _ => x.signum() * (x.abs() * p * (1.0 + f64::EPSILON)).floor() / p,
                })
            }
            "CEILING" | "FLOOR" => {
                n(args, 2, 2)?;
                let (x, s) = (self.number_arg(&args[0])?, self.number_arg(&args[1])?);
                if s == 0.0 {
                    return Ok(Value::Number(0.0));
                }
                if x > 0.0 && s < 0.0 {
                    return Err(ErrorKind::Num);
                }
                let q = x / s;
                num(if name == "CEILING" {
                    q.ceil()
                } else {
                    q.floor()
                } * s)
            }

            // Logic
            "IF" => {
                n(args, 2, 3)?;
                let c = to_bool(&self.scalar(&args[0])?)?;
                if c {
                    self.eval(&args[1])
                } else if args.len() == 3 {
                    self.eval(&args[2])
                } else {
                    Value::Bool(false)
                }
            }
            "IFERROR" => {
                n(args, 2, 2)?;
                match self.eval(&args[0]) {
                    Value::Error(_) => self.eval(&args[1]),
                    v => v,
                }
            }
            "IFNA" => {
                n(args, 2, 2)?;
                match self.eval(&args[0]) {
                    Value::Error(ErrorKind::Na) => self.eval(&args[1]),
                    v => v,
                }
            }
            "AND" | "OR" => {
                if args.is_empty() {
                    return Err(ErrorKind::Value);
                }
                let mut vals = Vec::new();
                for a in args {
                    match self.arg(a) {
                        Arg::Scalar(v) => vals.push(to_bool(&v)?),
                        Arg::Cell(v) => match v {
                            Value::Error(e) => return Err(e),
                            Value::Empty | Value::Text(_) => {}
                            v => vals.push(to_bool(&v)?),
                        },
                        Arg::Range(p, q) => {
                            for (_, v) in self.range_cells(p, q) {
                                match v {
                                    Value::Error(e) => return Err(e),
                                    Value::Text(_) => {}
                                    v => vals.push(to_bool(&v)?),
                                }
                            }
                        }
                    }
                }
                if vals.is_empty() {
                    return Err(ErrorKind::Value);
                }
                Value::Bool(if name == "AND" {
                    vals.iter().all(|b| *b)
                } else {
                    vals.iter().any(|b| *b)
                })
            }
            "NOT" => {
                n(args, 1, 1)?;
                Value::Bool(!to_bool(&self.scalar(&args[0])?)?)
            }
            "TRUE" => {
                n(args, 0, 0)?;
                Value::Bool(true)
            }
            "FALSE" => {
                n(args, 0, 0)?;
                Value::Bool(false)
            }

            // Information
            "ISNUMBER" | "ISTEXT" | "ISBLANK" | "ISERROR" | "ISNA" | "ISLOGICAL" => {
                n(args, 1, 1)?;
                let v = match self.arg(&args[0]) {
                    Arg::Scalar(v) | Arg::Cell(v) => v,
                    Arg::Range(a, b) if a == b => self.value_at(a),
                    Arg::Range(..) => return Err(ErrorKind::Value),
                };
                Value::Bool(match name {
                    "ISNUMBER" => matches!(v, Value::Number(_)),
                    "ISTEXT" => matches!(v, Value::Text(_)),
                    "ISBLANK" => v == Value::Empty,
                    "ISERROR" => v.is_error(),
                    "ISLOGICAL" => matches!(v, Value::Bool(_)),
                    _ => v == Value::Error(ErrorKind::Na),
                })
            }
            "NA" => {
                n(args, 0, 0)?;
                Value::Error(ErrorKind::Na)
            }

            // Text
            "LEN" => {
                n(args, 1, 1)?;
                Value::Number(self.text_arg(&args[0])?.chars().count() as f64)
            }
            "UPPER" | "LOWER" => {
                n(args, 1, 1)?;
                let t = self.text_arg(&args[0])?;
                Value::Text(if name == "UPPER" {
                    t.to_uppercase()
                } else {
                    t.to_lowercase()
                })
            }
            "TRIM" => {
                n(args, 1, 1)?;
                Value::Text(
                    self.text_arg(&args[0])?
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            }
            "LEFT" | "RIGHT" => {
                n(args, 1, 2)?;
                let t = self.text_arg(&args[0])?;
                let k = if args.len() == 2 {
                    self.int_arg(&args[1])?
                } else {
                    1
                };
                if k < 0 {
                    return Err(ErrorKind::Value);
                }
                let k = k as usize;
                let chars: Vec<char> = t.chars().collect();
                let k = k.min(chars.len());
                Value::Text(if name == "LEFT" {
                    chars[..k].iter().collect()
                } else {
                    chars[chars.len() - k..].iter().collect()
                })
            }
            "MID" => {
                n(args, 3, 3)?;
                let t = self.text_arg(&args[0])?;
                let (start, len) = (self.int_arg(&args[1])?, self.int_arg(&args[2])?);
                if start < 1 || len < 0 {
                    return Err(ErrorKind::Value);
                }
                Value::Text(
                    t.chars()
                        .skip(start as usize - 1)
                        .take(len as usize)
                        .collect(),
                )
            }
            "CONCAT" | "CONCATENATE" => {
                let mut s = String::new();
                for v in self.flatten(args) {
                    s += &to_text(&v)?;
                }
                Value::Text(s)
            }
            "TEXTJOIN" => {
                if args.len() < 3 {
                    return Err(ErrorKind::Value);
                }
                let delim = self.text_arg(&args[0])?;
                let skip_empty = to_bool(&self.scalar(&args[1])?)?;
                let mut parts = Vec::new();
                for v in self.flatten(&args[2..]) {
                    let t = to_text(&v)?;
                    if !(skip_empty && t.is_empty()) {
                        parts.push(t);
                    }
                }
                Value::Text(parts.join(&delim))
            }
            "SUBSTITUTE" => {
                n(args, 3, 4)?;
                let (t, old, new) = (
                    self.text_arg(&args[0])?,
                    self.text_arg(&args[1])?,
                    self.text_arg(&args[2])?,
                );
                if old.is_empty() {
                    return Ok(Value::Text(t));
                }
                if args.len() == 4 {
                    let which = self.int_arg(&args[3])?;
                    if which < 1 {
                        return Err(ErrorKind::Value);
                    }
                    let mut out = String::new();
                    let (mut seen, mut rest) = (0, t.as_str());
                    while let Some(i) = rest.find(&old) {
                        seen += 1;
                        out.push_str(&rest[..i]);
                        out.push_str(if seen == which { &new } else { &old });
                        rest = &rest[i + old.len()..];
                    }
                    out.push_str(rest);
                    Value::Text(out)
                } else {
                    Value::Text(t.replace(&old, &new))
                }
            }
            "FIND" | "SEARCH" => {
                n(args, 2, 3)?;
                let (needle, hay) = (self.text_arg(&args[0])?, self.text_arg(&args[1])?);
                let start = if args.len() == 3 {
                    self.int_arg(&args[2])?
                } else {
                    1
                };
                if start < 1 {
                    return Err(ErrorKind::Value);
                }
                let (needle, hay) = if name == "SEARCH" {
                    (needle.to_lowercase(), hay.to_lowercase())
                } else {
                    (needle, hay)
                };
                let hay_chars: Vec<char> = hay.chars().collect();
                let skip = (start as usize - 1).min(hay_chars.len());
                let tail: String = hay_chars[skip..].iter().collect();
                match tail.find(&needle) {
                    Some(byte) => Value::Number((skip + tail[..byte].chars().count() + 1) as f64),
                    None => return Err(ErrorKind::Value),
                }
            }
            "REPT" => {
                n(args, 2, 2)?;
                let (t, k) = (self.text_arg(&args[0])?, self.int_arg(&args[1])?);
                if k < 0 || t.chars().count() as i64 * k > 32_767 {
                    return Err(ErrorKind::Value);
                }
                Value::Text(t.repeat(k as usize))
            }
            "EXACT" => {
                n(args, 2, 2)?;
                Value::Bool(self.text_arg(&args[0])? == self.text_arg(&args[1])?)
            }
            "VALUE" => {
                n(args, 1, 1)?;
                match self.scalar(&args[0])? {
                    Value::Number(x) => Value::Number(x),
                    Value::Text(s) => Value::Number(parse_number(&s).ok_or(ErrorKind::Value)?),
                    Value::Empty => Value::Number(0.0),
                    _ => return Err(ErrorKind::Value),
                }
            }

            // Conditional aggregates
            "SUMIF" | "COUNTIF" | "AVERAGEIF" => {
                let counting = name == "COUNTIF";
                n(args, 2, if counting { 2 } else { 3 })?;
                let (a, b) = self.range_arg(&args[0])?;
                let crit = criteria(&self.scalar(&args[1])?);
                let sum_origin = if args.len() == 3 {
                    self.range_arg(&args[2])?.0
                } else {
                    a
                };
                let (mut total, mut hits) = (0.0, 0usize);
                // Blank cells can match too (e.g. "<>x"), so scan the whole rectangle when it is small.
                let cells: Vec<(Key, Value)> =
                    if crit.negate && (b.0 - a.0 + 1) as u64 * (b.1 - a.1 + 1) as u64 <= 100_000 {
                        (a.0..=b.0)
                            .flat_map(|r| (a.1..=b.1).map(move |c| (r, c)))
                            .map(|k| (k, self.value_at(k)))
                            .collect()
                    } else {
                        self.range_cells(a, b)
                    };
                for (k, v) in cells {
                    if !crit.matches(&v) {
                        continue;
                    }
                    hits += 1;
                    if !counting {
                        let src = (sum_origin.0 + (k.0 - a.0), sum_origin.1 + (k.1 - a.1));
                        if let Value::Number(x) = self.value_at(src) {
                            total += x;
                        }
                    }
                }
                match name {
                    "COUNTIF" => Value::Number(hits as f64),
                    "SUMIF" => Value::Number(total),
                    _ if hits == 0 => return Err(ErrorKind::DivZero),
                    _ => Value::Number(total / hits as f64),
                }
            }

            // Lookup
            "VLOOKUP" | "HLOOKUP" => {
                n(args, 3, 4)?;
                let key = self.scalar(&args[0])?;
                let (a, b) = self.range_arg(&args[1])?;
                let idx = self.int_arg(&args[2])?;
                let approx = if args.len() == 4 {
                    to_bool(&self.scalar(&args[3])?)?
                } else {
                    true
                };
                let mut m = self.matrix(a, b);
                if name == "HLOOKUP" {
                    // Work on the transpose: rows of the table become columns.
                    let width = m.iter().map(Vec::len).max().unwrap_or(0);
                    m = (0..width)
                        .map(|c| {
                            m.iter()
                                .map(|r| r.get(c).cloned().unwrap_or_default())
                                .collect()
                        })
                        .collect();
                }
                if idx < 1 {
                    return Err(ErrorKind::Value);
                }
                let width = (if name == "VLOOKUP" {
                    b.1 - a.1
                } else {
                    b.0 - a.0
                } + 1) as i64;
                if idx > width {
                    return Err(ErrorKind::Ref);
                }
                let mut found = None;
                for (i, row) in m.iter().enumerate() {
                    let probe = row.first().cloned().unwrap_or_default();
                    if approx {
                        if probe == Value::Empty || rank(&probe) != rank(&key) {
                            continue;
                        }
                        if compare(&probe, &key) == Ordering::Greater {
                            break;
                        }
                        found = Some(i);
                    } else if values_equal(&key, &probe) {
                        found = Some(i);
                        break;
                    }
                }
                let i = found.ok_or(ErrorKind::Na)?;
                m[i].get(idx as usize - 1).cloned().unwrap_or_default()
            }
            "MATCH" => {
                n(args, 2, 3)?;
                let key = self.scalar(&args[0])?;
                let (a, b) = self.range_arg(&args[1])?;
                let kind = if args.len() == 3 {
                    self.int_arg(&args[2])?
                } else {
                    1
                };
                if a.0 != b.0 && a.1 != b.1 {
                    return Err(ErrorKind::Na);
                }
                let vector: Vec<Value> =
                    self.matrix(a, b).into_iter().flatten().collect::<Vec<_>>();
                // matrix() flattens row-major, which is the right order for a single row or column.
                let mut found = None;
                for (i, v) in vector.iter().enumerate() {
                    match kind {
                        0 => {
                            if values_equal(&key, v) {
                                found = Some(i);
                                break;
                            }
                        }
                        k => {
                            if *v == Value::Empty || rank(v) != rank(&key) {
                                continue;
                            }
                            let o = compare(v, &key);
                            let past = if k > 0 {
                                o == Ordering::Greater
                            } else {
                                o == Ordering::Less
                            };
                            if past {
                                break;
                            }
                            found = Some(i);
                        }
                    }
                }
                Value::Number(found.ok_or(ErrorKind::Na)? as f64 + 1.0)
            }
            "INDEX" => {
                n(args, 2, 3)?;
                let (a, b) = self.range_arg(&args[0])?;
                let (rows, cols) = ((b.0 - a.0 + 1) as i64, (b.1 - a.1 + 1) as i64);
                let first = self.int_arg(&args[1])?;
                let (r, c) = if args.len() == 3 {
                    (first, self.int_arg(&args[2])?)
                } else if rows == 1 {
                    (1, first) // a single row: the index runs along it
                } else {
                    (first, 1)
                };
                if r < 1 || c < 1 {
                    return Err(ErrorKind::Value);
                }
                if r > rows || c > cols {
                    return Err(ErrorKind::Ref);
                }
                self.value_at((a.0 + r as u32 - 1, a.1 + c as u32 - 1))
            }
            _ => return Err(ErrorKind::Name),
        })
    }
}
