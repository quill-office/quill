//! Cell formatting.

use crate::value::{format_fixed, format_general};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// Numbers to the right, everything else to the left.
    #[default]
    Auto,
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NumFmt {
    #[default]
    General,
    Fixed {
        decimals: u8,
        thousands: bool,
    },
    Percent {
        decimals: u8,
    },
    Currency {
        decimals: u8,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub align: Align,
    /// Text colour as 0xRRGGBB.
    pub color: Option<u32>,
    /// Background colour as 0xRRGGBB.
    pub fill: Option<u32>,
    pub fmt: NumFmt,
}

impl Style {
    pub fn is_default(&self) -> bool {
        *self == Style::default()
    }
}

/// A number as text under `fmt`.
pub fn format_number(n: f64, fmt: NumFmt) -> String {
    match fmt {
        NumFmt::General => format_general(n),
        NumFmt::Fixed {
            decimals,
            thousands,
        } => format_fixed(n, decimals, thousands),
        NumFmt::Percent { decimals } => format!("{}%", format_fixed(n * 100.0, decimals, false)),
        NumFmt::Currency { decimals } => {
            let body = format_fixed(n.abs(), decimals, true);
            if body.chars().any(|c| c.is_ascii_digit() && c != '0') && n < 0.0 {
                format!("-${body}")
            } else {
                format!("${body}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formats() {
        assert_eq!(format_number(1234.5, NumFmt::General), "1234.5");
        assert_eq!(
            format_number(
                1234.5,
                NumFmt::Fixed {
                    decimals: 1,
                    thousands: true
                }
            ),
            "1,234.5"
        );
        assert_eq!(
            format_number(0.256, NumFmt::Percent { decimals: 1 }),
            "25.6%"
        );
        assert_eq!(format_number(0.5, NumFmt::Percent { decimals: 0 }), "50%");
        assert_eq!(
            format_number(-1234.5, NumFmt::Currency { decimals: 2 }),
            "-$1,234.50"
        );
        assert_eq!(
            format_number(1234.5, NumFmt::Currency { decimals: 2 }),
            "$1,234.50"
        );
        assert_eq!(
            format_number(-0.001, NumFmt::Currency { decimals: 2 }),
            "$0.00",
            "no negative zero"
        );
        assert!(
            Style::default().is_default()
                && !Style {
                    bold: true,
                    ..Default::default()
                }
                .is_default()
        );
    }
}
