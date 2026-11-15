/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::MetricFormat;
use std::fmt::Write;

const NO_VALUE: &str = "\u{2013}";
const COMPACT_SUFFIXES: [&str; 4] = ["K", "M", "B", "T"];
const SIZE_UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
const KIB: f64 = 1024.0;

pub fn format_value(value: f64, format: MetricFormat, out: &mut String) {
    if !value.is_finite() {
        out.push_str(NO_VALUE);
        return;
    }
    match format {
        MetricFormat::Number => compact_number(value, out),
        MetricFormat::Bytes => compact_size(value, out),
        MetricFormat::Duration => elapsed(value, out),
        MetricFormat::Percent => {
            number(value, 1, 1, out);
            out.push('%');
        }
        MetricFormat::Unknown => {
            let _ = write!(out, "{value}");
        }
    }
}

fn compact_number(value: f64, out: &mut String) {
    let abs = value.abs();
    if abs < 1000.0 {
        number(value, 0, 1, out);
        return;
    }
    let mut tier = 0usize;
    let mut scaled = abs / 1000.0;
    while tier + 1 < COMPACT_SUFFIXES.len() && round_to(scaled, 1) >= 1000.0 {
        tier += 1;
        scaled /= 1000.0;
    }
    number(scaled.copysign(value), 0, 1, out);
    out.push_str(COMPACT_SUFFIXES[tier]);
}

fn compact_size(bytes: f64, out: &mut String) {
    let abs = bytes.abs();
    let mut factor = KIB.powi(4);
    for (tier, unit) in SIZE_UNITS.iter().enumerate().rev() {
        let scaled = abs / factor;
        let digits = if scaled < 9.95 && tier > 0 { 1 } else { 0 };
        let rounded = round_to(scaled, digits);
        if rounded >= 1.0 || tier == 0 {
            number(rounded.copysign(bytes), digits, digits, out);
            out.push(' ');
            out.push_str(unit);
            return;
        }
        factor /= KIB;
    }
}

fn elapsed(ms: f64, out: &mut String) {
    if ms <= 0.0 {
        out.push_str("0 ms");
        return;
    }
    let whole_ms = ms.round();
    if whole_ms < 1000.0 {
        number(whole_ms, 0, 0, out);
        out.push_str(" ms");
        return;
    }
    let seconds = round_to(ms / 1000.0, 1);
    if seconds < 60.0 {
        number(seconds, 1, 1, out);
        out.push_str(" s");
        return;
    }
    let total_seconds = ((ms / 1000.0).floor() as u64).max(60);
    let total_minutes = total_seconds / 60;
    let s = total_seconds % 60;
    let _ = if total_minutes < 60 {
        if s > 0 {
            write!(out, "{total_minutes} m {s} s")
        } else {
            write!(out, "{total_minutes} m")
        }
    } else {
        let (h, m) = (total_minutes / 60, total_minutes % 60);
        number(h as f64, 0, 0, out);
        if m > 0 {
            write!(out, " h {m} m")
        } else {
            out.write_str(" h")
        }
    };
}

fn round_to(value: f64, digits: usize) -> f64 {
    let factor = 10f64.powi(i32::try_from(digits).unwrap_or(0));
    (value * factor).round() / factor
}

fn number(value: f64, min_digits: usize, max_digits: usize, out: &mut String) {
    let rounded = round_to(value, max_digits);
    if rounded < 0.0 {
        out.push('-');
    }
    let mut digits = String::with_capacity(24);
    let _ = write!(digits, "{:.max_digits$}", rounded.abs());
    let (int, frac) = digits.split_once('.').unwrap_or((&digits, ""));
    let keep = frac
        .trim_end_matches('0')
        .len()
        .max(min_digits)
        .min(frac.len());
    let len = int.len();
    for (i, c) in int.char_indices() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(frac) = frac.get(..keep).filter(|f| !f.is_empty()) {
        out.push('.');
        out.push_str(frac);
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/view/metrics/format/tests.rs"]
mod tests;
