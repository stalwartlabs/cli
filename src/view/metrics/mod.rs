/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod format;
pub mod plan;
pub mod record;
mod reduce;

use crate::schema::Polarity;
use crate::view::time::UtcTimestamp;
use serde::{Deserialize, Serialize};
use std::fmt;

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;
pub const CUSTOM_BUCKETS: u16 = 60;
pub const SPARKLINE_BUCKETS: u16 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RangePreset {
    #[default]
    #[serde(rename = "24h")]
    Day,
    #[serde(rename = "7d")]
    Week,
    #[serde(rename = "30d")]
    Month,
    #[serde(rename = "90d")]
    Quarter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub from: UtcTimestamp,
    pub to: UtcTimestamp,
    pub buckets: u16,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DashboardData {
    pub items: Vec<ItemData>,
    pub nodes: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemData {
    Stat(StatData),
    Chart(ChartData),
    Headline { ratio: Option<f64> },
    Ranking(Vec<RankEntry>),
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatData {
    pub value: f64,
    pub sparkline: Option<Box<[f64]>>,
    pub delta: Option<Delta>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartData {
    pub series: Vec<Box<[Option<f64>]>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RankEntry {
    pub series: usize,
    pub value: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Delta {
    pub pct: f64,
    pub direction: Direction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Neutral,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sentiment {
    Good,
    Bad,
    Neutral,
}

impl RangePreset {
    pub const ALL: [RangePreset; 4] = [
        RangePreset::Day,
        RangePreset::Week,
        RangePreset::Month,
        RangePreset::Quarter,
    ];

    pub fn span_ms(self) -> i64 {
        DAY_MS
            * match self {
                RangePreset::Day => 1,
                RangePreset::Week => 7,
                RangePreset::Month => 30,
                RangePreset::Quarter => 90,
            }
    }

    pub fn buckets(self) -> u16 {
        match self {
            RangePreset::Day => 48,
            RangePreset::Week => 56,
            RangePreset::Month => 60,
            RangePreset::Quarter => 90,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RangePreset::Day => "Last 24 hours",
            RangePreset::Week => "Last 7 days",
            RangePreset::Month => "Last 30 days",
            RangePreset::Quarter => "Last 90 days",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            RangePreset::Day => "24h",
            RangePreset::Week => "7d",
            RangePreset::Month => "30d",
            RangePreset::Quarter => "90d",
        }
    }
}

impl Window {
    pub fn write_half(&self, out: &mut impl fmt::Write) -> fmt::Result {
        let half = (self.to.millis() - self.from.millis()) / 2;
        if half >= DAY_MS && half % DAY_MS == 0 {
            write!(out, "{}d", half / DAY_MS)
        } else {
            write!(out, "{}h", (half + HOUR_MS / 2) / HOUR_MS)
        }
    }

    pub fn preset(preset: RangePreset, now: UtcTimestamp) -> Self {
        Window {
            from: now.add_millis(-preset.span_ms()),
            to: now,
            buckets: preset.buckets(),
        }
    }

    pub fn custom(from: UtcTimestamp, to: UtcTimestamp) -> Option<Self> {
        (from < to).then_some(Window {
            from,
            to,
            buckets: CUSTOM_BUCKETS,
        })
    }

    pub fn with_buckets(self, buckets: u16) -> Self {
        Window {
            buckets: buckets.max(1),
            ..self
        }
    }

    pub fn span_ms(&self) -> i64 {
        self.to.millis() - self.from.millis()
    }

    pub fn bucket_of(&self, at: UtcTimestamp) -> Option<usize> {
        let span = self.span_ms();
        let offset = at.millis() - self.from.millis();
        if span <= 0 || offset < 0 || offset > span {
            return None;
        }
        let buckets = i128::from(self.buckets.max(1));
        let index = (i128::from(offset) * buckets / i128::from(span)).min(buckets - 1);
        usize::try_from(index).ok()
    }

    pub fn bucket_center(&self, index: usize) -> UtcTimestamp {
        self.bucket_at(index, 1)
    }

    fn bucket_at(&self, index: usize, half_steps: i128) -> UtcTimestamp {
        let buckets = i128::from(self.buckets.max(1));
        let index = i128::try_from(index).unwrap_or(i128::MAX / 4);
        let offset = i128::from(self.span_ms()) * (2 * index + half_steps) / (2 * buckets);
        UtcTimestamp(
            self.from
                .millis()
                .saturating_add(i64::try_from(offset).unwrap_or(i64::MAX)),
        )
    }
}

impl Delta {
    pub fn between(first: f64, second: f64) -> Option<Self> {
        if first == 0.0 {
            return None;
        }
        let pct = (second - first) / first.abs() * 100.0;
        let direction = match pct {
            p if p > 0.0 => Direction::Up,
            p if p < 0.0 => Direction::Down,
            _ => Direction::Neutral,
        };
        Some(Delta {
            pct: (pct * 10.0).round() / 10.0,
            direction,
        })
    }

    pub fn sentiment(&self, polarity: Option<Polarity>) -> Sentiment {
        let higher_is_better = match polarity {
            None | Some(Polarity::HigherIsBetter) => true,
            Some(Polarity::LowerIsBetter) => false,
            Some(Polarity::Neutral | Polarity::Unknown) => return Sentiment::Neutral,
        };
        match self.direction {
            Direction::Neutral => Sentiment::Neutral,
            Direction::Up if higher_is_better => Sentiment::Good,
            Direction::Down if !higher_is_better => Sentiment::Good,
            _ => Sentiment::Bad,
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/view/metrics/tests.rs"]
mod tests;
