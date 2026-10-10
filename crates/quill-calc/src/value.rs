//! Cell values, errors and number formatting.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    DivZero,
    Ref,
    Name,
    Value,
    Na,
    Num,
    /// A formula that (indirectly) refers to itself.
    Circular,
    /// A formula that could not be parsed.
    Parse,
}

impl ErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            ErrorKind::DivZero => "#DIV/0!",
            ErrorKind::Ref => "#REF!",
            ErrorKind::Name => "#NAME?",
            ErrorKind::Value => "#VALUE!",
            ErrorKind::Na => "#N/A",
            ErrorKind::Num => "#NUM!",
            ErrorKind::Circular => "#CIRC!",
            ErrorKind::Parse => "#ERROR!",
        }
    }

    pub fn from_code(s: &str) -> Option<ErrorKind> {
        const ALL: [ErrorKind; 8] = [
            ErrorKind::DivZero,
            ErrorKind::Ref,
            ErrorKind::Name,
            ErrorKind::Value,
            ErrorKind::Na,
            ErrorKind::Num,
            ErrorKind::Circular,
            ErrorKind::Parse,
        ];
        ALL.into_iter().find(|e| e.code().eq_ignore_ascii_case(s))
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    Error(ErrorKind),
}

impl Value {
    pub fn is_error(&self) -> bool {
        matches!(self, Value::Error(_))
    }

    /// Text as a formula would see it (`&`, `LEN`, …).
    pub fn to_text(&self) -> String {
        match self {
            Value::Empty => String::new(),
            Value::Number(n) => format_general(*n),
            Value::Text(s) => s.clone(),
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
            Value::Error(e) => e.code().to_string(),
        }
    }
}

/// Excel's "General" format for a number: up to 15 significant digits, no
/// trailing zeros, scientific notation for very large or small magnitudes.
pub fn format_general(n: f64) -> String {
    if n == 0.0 {
        return "0".into();
    }
    if !n.is_finite() {
        return ErrorKind::Num.code().into();
    }
    let abs = n.abs();
    if (1e-9..1e15).contains(&abs) {
        if n.fract() == 0.0 {
            return format!("{n:.0}");
        }
        let exp = abs.log10().floor() as i32;
        let decimals = (14 - exp).clamp(0, 20) as usize;
        let s = format!("{n:.decimals$}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        return if s == "-0" { "0".into() } else { s.into() };
    }
    // Scientific: 15 significant digits, trimmed, like 1.5E+20.
    let s = format!("{n:.14e}");
    let (mant, exp) = s.split_once('e').expect("exponent form");
    let mant = mant.trim_end_matches('0').trim_end_matches('.');
    let exp: i32 = exp.parse().expect("integer exponent");
    format!("{mant}E{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
}

/// Round to `digits` decimals, halves away from zero, as spreadsheets do.
/// (Plain `f64::round` on `x * 10^d` mishandles binary noise: 1.005 would
/// give 1.00 where a spreadsheet shows 1.01.) Negative `digits` round to
/// tens, hundreds, …
pub fn round_half_away(x: f64, digits: i32) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let p = 10f64.powi(digits.clamp(-300, 300));
    let scaled = x.abs() * p * (1.0 + f64::EPSILON);
    let r = scaled.round() / p;
    if x < 0.0 {
        -r
    } else {
        r
    }
}

/// Fixed decimals with optional thousands separators.
pub fn format_fixed(n: f64, decimals: u8, thousands: bool) -> String {
    let n = round_half_away(n, decimals as i32);
    let s = format!("{:.*}", decimals as usize, n.abs());
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), Some(f.to_string())),
        None => (s, None),
    };
    let int = if thousands {
        let digits: Vec<char> = int.chars().collect();
        let mut out = String::new();
        for (i, c) in digits.iter().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(*c);
        }
        out
    } else {
        int
    };
    let body = match frac {
        Some(f) => format!("{int}.{f}"),
        None => int,
    };
    // Negative numbers that round to zero lose their sign.
    let negative = n < 0.0 && body.chars().any(|c| c.is_ascii_digit() && c != '0');
    if negative {
        format!("-{body}")
    } else {
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_format_matches_spreadsheet_conventions() {
        assert_eq!(format_general(0.0), "0");
        assert_eq!(format_general(42.0), "42");
        assert_eq!(format_general(-7.0), "-7");
        assert_eq!(format_general(0.1 + 0.2), "0.3", "binary noise is hidden");
        assert_eq!(format_general(1.0 / 3.0), "0.333333333333333");
        assert_eq!(format_general(123456.789), "123456.789");
        assert_eq!(format_general(1e15), "1E+15");
        assert_eq!(format_general(1.5e20), "1.5E+20");
        assert_eq!(format_general(2.5e-10), "2.5E-10");
        assert_eq!(format_general(-1.25e-12), "-1.25E-12");
        assert_eq!(format_general(999999999999999.0), "999999999999999");
        assert_eq!(format_general(f64::INFINITY), "#NUM!");
        assert_eq!(format_general(-0.0), "0");
    }

    #[test]
    fn fixed_format_rounds_groups_and_keeps_sign() {
        assert_eq!(format_fixed(1234567.891, 2, true), "1,234,567.89");
        assert_eq!(format_fixed(-1234.5, 0, true), "-1,235");
        assert_eq!(format_fixed(0.5, 0, false), "1");
        assert_eq!(format_fixed(-0.001, 2, false), "0.00", "no negative zero");
        assert_eq!(format_fixed(999.999, 2, true), "1,000.00");
        assert_eq!(format_fixed(12.0, 3, false), "12.000");
        assert_eq!(format_fixed(100.0, 0, true), "100");
        assert_eq!(
            format_fixed(1.005, 2, false),
            "1.01",
            "binary noise does not push a half down"
        );
        assert_eq!(format_fixed(2.675, 2, false), "2.68");
        assert_eq!(format_fixed(-2.5, 0, false), "-3");
    }

    #[test]
    fn rounding_helper_handles_digits_signs_and_infinities() {
        assert_eq!(round_half_away(2.5, 0), 3.0);
        assert_eq!(round_half_away(-2.5, 0), -3.0);
        assert_eq!(round_half_away(1234.5678, 2), 1234.57);
        assert_eq!(round_half_away(1234.5678, -2), 1200.0);
        assert_eq!(round_half_away(1250.0, -2), 1300.0);
        assert_eq!(round_half_away(0.0, 3), 0.0);
        assert!(round_half_away(f64::NAN, 1).is_nan());
        assert_eq!(round_half_away(f64::INFINITY, 1), f64::INFINITY);
    }

    #[test]
    fn errors_round_trip_through_their_codes() {
        for e in [
            ErrorKind::DivZero,
            ErrorKind::Ref,
            ErrorKind::Name,
            ErrorKind::Value,
            ErrorKind::Na,
            ErrorKind::Num,
            ErrorKind::Circular,
            ErrorKind::Parse,
        ] {
            assert_eq!(ErrorKind::from_code(e.code()), Some(e));
        }
        assert_eq!(ErrorKind::from_code("#div/0!"), Some(ErrorKind::DivZero));
        assert_eq!(ErrorKind::from_code("#nope"), None);
        assert_eq!(Value::Bool(true).to_text(), "TRUE");
        assert_eq!(Value::Error(ErrorKind::Na).to_text(), "#N/A");
    }
}
