/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantity {
    Size,
    Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    pub label: &'static str,
    pub factor: u64,
}

const SIZE_UNITS: &[Unit] = &[
    Unit {
        label: "B",
        factor: 1,
    },
    Unit {
        label: "KB",
        factor: 1 << 10,
    },
    Unit {
        label: "MB",
        factor: 1 << 20,
    },
    Unit {
        label: "GB",
        factor: 1 << 30,
    },
    Unit {
        label: "TB",
        factor: 1 << 40,
    },
];

const DURATION_UNITS: &[Unit] = &[
    Unit {
        label: "ms",
        factor: 1,
    },
    Unit {
        label: "s",
        factor: 1_000,
    },
    Unit {
        label: "min",
        factor: 60_000,
    },
    Unit {
        label: "h",
        factor: 3_600_000,
    },
    Unit {
        label: "d",
        factor: 86_400_000,
    },
];

impl Quantity {
    pub fn units(self) -> &'static [Unit] {
        match self {
            Quantity::Size => SIZE_UNITS,
            Quantity::Duration => DURATION_UNITS,
        }
    }

    pub fn best_unit(self, raw: u64) -> usize {
        self.units()
            .iter()
            .rposition(|unit| raw != 0 && raw.is_multiple_of(unit.factor))
            .unwrap_or(0)
    }

    pub fn empty_unit(self) -> usize {
        let name = match self {
            Quantity::Size => "MB",
            Quantity::Duration => "s",
        };
        self.unit_named(name).unwrap_or(0)
    }

    pub fn unit_named(self, name: &str) -> Option<usize> {
        let index = match self {
            Quantity::Size => hashify::map_ignore_case!(name.as_bytes(), usize,
                "b" => 0, "kb" => 1, "k" => 1, "kib" => 1, "mb" => 2, "m" => 2, "mib" => 2,
                "gb" => 3, "g" => 3, "gib" => 3, "tb" => 4, "t" => 4, "tib" => 4,
            ),
            Quantity::Duration => hashify::map_ignore_case!(name.as_bytes(), usize,
                "ms" => 0, "s" => 1, "sec" => 1, "min" => 2, "m" => 2, "h" => 3, "hr" => 3, "d" => 4, "day" => 4, "days" => 4,
            ),
        };
        index.copied()
    }

    pub fn write_amount(self, raw: u64, unit: usize, out: &mut String) {
        if let Some(unit) = self.units().get(unit) {
            write_amount(raw, unit.factor, out);
        }
    }

    pub fn parse(self, text: &str, default_unit: usize) -> Result<u64, UnitError> {
        let text = text.trim();
        let number = text.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == ' ');
        let suffix = text.strip_prefix(number).unwrap_or_default().trim();
        let amount = parse_decimal(number.trim()).ok_or(UnitError::NotANumber)?;
        let unit = if suffix.is_empty() {
            default_unit
        } else {
            self.unit_named(suffix).ok_or(UnitError::UnknownUnit)?
        };
        let factor = self.units().get(unit).ok_or(UnitError::UnknownUnit)?.factor;
        amount.scale(factor).ok_or(UnitError::TooLarge)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitError {
    NotANumber,
    UnknownUnit,
    TooLarge,
}

impl UnitError {
    pub fn message(self) -> &'static str {
        match self {
            UnitError::NotANumber => "not a number",
            UnitError::UnknownUnit => "unknown unit",
            UnitError::TooLarge => "value too large",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Decimal {
    whole: u64,
    hundredths: u64,
}

impl Decimal {
    fn scale(self, factor: u64) -> Option<u64> {
        let fraction = self.hundredths.checked_mul(factor)?.checked_add(50)? / 100;
        self.whole.checked_mul(factor)?.checked_add(fraction)
    }
}

fn parse_decimal(text: &str) -> Option<Decimal> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    if !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut digits = fraction.bytes().map(|b| u64::from(b - b'0'));
    let tenths = digits.next().unwrap_or(0);
    let hundredths = digits.next().unwrap_or(0);
    let round_up = u64::from(digits.next().is_some_and(|d| d >= 5));
    Some(Decimal {
        whole,
        hundredths: tenths * 10 + hundredths + round_up,
    })
}

fn write_amount(raw: u64, factor: u64, out: &mut String) {
    let whole = raw / factor;
    let rest = raw % factor;
    let hundredths = (u128::from(rest) * 100 + u128::from(factor) / 2) / u128::from(factor);
    let (whole, hundredths) = if hundredths >= 100 {
        (whole + 1, 0)
    } else {
        (whole, hundredths)
    };
    let _ = write!(out, "{whole}");
    if hundredths > 0 {
        if hundredths % 10 == 0 {
            let _ = write!(out, ".{}", hundredths / 10);
        } else {
            let _ = write!(out, ".{hundredths:02}");
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/form/editor/units/tests.rs"]
mod tests;
