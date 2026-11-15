/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::fmt::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

const MS_PER_SECOND: i64 = 1000;
const MS_PER_MINUTE: i64 = 60 * MS_PER_SECOND;
const MS_PER_DAY: i64 = 24 * 60 * MS_PER_MINUTE;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct UtcTimestamp(pub i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
}

impl UtcTimestamp {
    pub fn from_system(time: SystemTime) -> Self {
        let millis = match time.duration_since(UNIX_EPOCH) {
            Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
            Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
        };
        UtcTimestamp(millis)
    }

    pub fn millis(self) -> i64 {
        self.0
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().as_bytes();
        let (date, rest) = parse_date(raw)?;
        let (sep, rest) = rest.split_first()?;
        if !matches!(sep, b'T' | b't' | b' ') {
            return None;
        }
        let (hour, rest) = digits(rest, 2)?;
        let rest = expect(rest, b':')?;
        let (minute, rest) = digits(rest, 2)?;
        let rest = expect(rest, b':')?;
        let (second, mut rest) = digits(rest, 2)?;
        let mut millis = 0;
        if let Some(fraction) = rest.strip_prefix(b".") {
            let len = fraction.iter().take_while(|b| b.is_ascii_digit()).count();
            let (frac, tail) = fraction.split_at_checked(len)?;
            if frac.is_empty() {
                return None;
            }
            millis = frac
                .iter()
                .chain(b"00")
                .take(3)
                .fold(0, |acc, d| acc * 10 + u32::from(d - b'0'));
            rest = tail;
        }
        let offset_minutes = match rest {
            [b'Z' | b'z'] => 0,
            [sign @ (b'+' | b'-'), tail @ ..] => {
                let (oh, tail) = digits(tail, 2)?;
                let tail = tail.strip_prefix(b":").unwrap_or(tail);
                let (om, tail) = digits(tail, 2)?;
                if !tail.is_empty() || oh > 23 || om > 59 {
                    return None;
                }
                let minutes = i64::from(oh * 60 + om);
                if *sign == b'-' { -minutes } else { minutes }
            }
            _ => return None,
        };
        if hour > 23 || minute > 59 || second > 60 {
            return None;
        }
        let local = date.0
            + i64::from(hour) * 60 * MS_PER_MINUTE
            + i64::from(minute) * MS_PER_MINUTE
            + i64::from(second) * MS_PER_SECOND
            + i64::from(millis);
        Some(UtcTimestamp(local - offset_minutes * MS_PER_MINUTE))
    }

    pub fn parse_date(raw: &str) -> Option<Self> {
        match parse_date(raw.trim().as_bytes())? {
            (date, []) => Some(date),
            _ => None,
        }
    }

    pub fn parse_minute(raw: &str) -> Option<Self> {
        let (date, rest) = parse_date(raw.trim().as_bytes())?;
        let (sep, rest) = rest.split_first()?;
        if !matches!(sep, b'T' | b't') {
            return None;
        }
        let (hour, rest) = digits(rest, 2)?;
        let rest = expect(rest, b':')?;
        let (minute, rest) = digits(rest, 2)?;
        if !rest.is_empty() || hour > 23 || minute > 59 {
            return None;
        }
        Some(UtcTimestamp(
            date.0 + i64::from(hour * 60 + minute) * MS_PER_MINUTE,
        ))
    }

    pub fn from_civil(year: i64, month: u32, day: u32) -> Option<Self> {
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        days_from_civil(year, month, day)
            .checked_mul(MS_PER_DAY)
            .map(UtcTimestamp)
    }

    pub fn civil(self) -> Civil {
        let days = self.0.div_euclid(MS_PER_DAY);
        let in_day = self.0.rem_euclid(MS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        let seconds = in_day / MS_PER_SECOND;
        Civil {
            year,
            month,
            day,
            hour: u32::try_from(seconds / 3600).unwrap_or(0),
            minute: u32::try_from(seconds / 60 % 60).unwrap_or(0),
            second: u32::try_from(seconds % 60).unwrap_or(0),
            millis: u32::try_from(in_day % MS_PER_SECOND).unwrap_or(0),
        }
    }

    pub fn day_start(self) -> Self {
        UtcTimestamp(self.0 - self.0.rem_euclid(MS_PER_DAY))
    }

    pub fn next_day_start(self) -> Self {
        UtcTimestamp(self.day_start().0.saturating_add(MS_PER_DAY))
    }

    pub fn add_millis(self, millis: i64) -> Self {
        UtcTimestamp(self.0.saturating_add(millis))
    }

    pub fn format_iso(self, out: &mut impl Write) -> fmt::Result {
        let c = self.civil();
        write!(
            out,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            c.year, c.month, c.day, c.hour, c.minute, c.second
        )?;
        if c.millis != 0 {
            write!(out, ".{:03}", c.millis)?;
        }
        out.write_char('Z')
    }

    pub fn format_minute(self, out: &mut impl Write) -> fmt::Result {
        let c = self.civil();
        write!(
            out,
            "{:04}-{:02}-{:02}T{:02}:{:02}",
            c.year, c.month, c.day, c.hour, c.minute
        )
    }

    pub fn format_date(self, out: &mut impl Write) -> fmt::Result {
        let c = self.civil();
        write!(out, "{:04}-{:02}-{:02}", c.year, c.month, c.day)
    }
}

fn parse_date(raw: &[u8]) -> Option<(UtcTimestamp, &[u8])> {
    let (sign, raw) = match raw {
        [b'-', rest @ ..] => (-1, rest),
        [b'+', rest @ ..] => (1, rest),
        _ => (1, raw),
    };
    let (year, rest) = digits(raw, 4)?;
    let rest = expect(rest, b'-')?;
    let (month, rest) = digits(rest, 2)?;
    let rest = expect(rest, b'-')?;
    let (day, rest) = digits(rest, 2)?;
    let date = UtcTimestamp::from_civil(sign * i64::from(year), month, day)?;
    Some((date, rest))
}

fn digits(raw: &[u8], len: usize) -> Option<(u32, &[u8])> {
    let (head, rest) = raw.split_at_checked(len)?;
    let value = head.iter().try_fold(0u32, |acc, b| {
        b.is_ascii_digit().then(|| acc * 10 + u32::from(b - b'0'))
    })?;
    Some((value, rest))
}

fn expect(raw: &[u8], byte: u8) -> Option<&[u8]> {
    match raw.split_first() {
        Some((first, rest)) if *first == byte => Some(rest),
        _ => None,
    }
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/view/time/tests.rs"]
mod tests;
