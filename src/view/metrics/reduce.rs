/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use super::Window;
use super::record::MetricRecord;
use crate::jmap::live::MetricKind;
use crate::schema::{Aggregate, MetricScope};
use crate::view::time::UtcTimestamp;
use std::mem;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Sum,
    Avg,
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Combine {
    Max,
    Sum,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub mode: Mode,
    pub combine: Combine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Span {
    Whole,
    Halves(Window),
    Buckets(Window),
}

#[derive(Clone, Copy, Debug, Default)]
struct Acc {
    sum: f64,
    n: u32,
}

#[derive(Clone, Copy, Debug)]
struct Latest {
    node: u64,
    metric: usize,
    at: Option<i64>,
    kind: MetricKind,
    count: f64,
    sum: f64,
}

#[derive(Clone, Debug, Default)]
struct Bucket {
    counters: Acc,
    histograms: Vec<(usize, f64, f64)>,
    gauges: Vec<(u64, Acc)>,
    latest: Vec<Latest>,
}

#[derive(Clone, Debug)]
struct SeriesTimes {
    node: u64,
    metric: usize,
    times: Vec<i64>,
}

#[derive(Clone, Debug)]
pub(super) struct Reducer {
    target: Target,
    span: Span,
    carry: bool,
    buckets: Box<[Bucket]>,
    series: Vec<SeriesTimes>,
}

impl Target {
    pub fn new(aggregate: Option<Aggregate>, scope: Option<MetricScope>) -> Self {
        Target {
            mode: match aggregate {
                Some(Aggregate::Avg) => Mode::Avg,
                Some(Aggregate::Last) => Mode::Last,
                _ => Mode::Sum,
            },
            combine: match scope {
                Some(MetricScope::Node) => Combine::Sum,
                _ => Combine::Max,
            },
        }
    }
}

impl Acc {
    fn add(&mut self, value: f64) {
        self.sum += value;
        self.n += 1;
    }

    fn value(self, mode: Mode) -> f64 {
        match mode {
            Mode::Avg if self.n > 0 => self.sum / f64::from(self.n),
            _ => self.sum,
        }
    }
}

impl Latest {
    fn of(record: &MetricRecord<'_>, metric: usize) -> Self {
        Latest {
            node: record.node_id(),
            metric,
            at: record.at.map(UtcTimestamp::millis),
            kind: record.kind,
            count: record.count,
            sum: record.sum,
        }
    }

    fn is_superseded_by(&self, at: Option<i64>) -> bool {
        !matches!((at, self.at), (Some(new), Some(old)) if new < old)
    }
}

impl Bucket {
    fn add(&mut self, node: u64, metric: usize, kind: MetricKind, count: f64, sum: f64) {
        match kind {
            MetricKind::Counter => self.counters.add(count),
            MetricKind::Histogram => {
                match self.histograms.iter_mut().find(|(m, _, _)| *m == metric) {
                    Some((_, c, s)) => {
                        *c += count;
                        *s += sum;
                    }
                    None => self.histograms.push((metric, count, sum)),
                }
            }
            MetricKind::Gauge => self.add_gauge(node, count),
            MetricKind::Unknown => {}
        }
    }

    fn add_gauge(&mut self, node: u64, value: f64) {
        match self.gauges.iter_mut().find(|(n, _)| *n == node) {
            Some((_, acc)) => acc.add(value),
            None => {
                let mut acc = Acc::default();
                acc.add(value);
                self.gauges.push((node, acc));
            }
        }
    }

    fn keep_latest(&mut self, latest: Latest) {
        match self
            .latest
            .iter_mut()
            .find(|l| l.node == latest.node && l.metric == latest.metric)
        {
            Some(prev) if prev.is_superseded_by(latest.at) => *prev = latest,
            Some(_) => {}
            None => self.latest.push(latest),
        }
    }

    fn reduce(&mut self, target: Target) -> Option<f64> {
        if target.mode == Mode::Last {
            for l in mem::take(&mut self.latest) {
                self.add(l.node, l.metric, l.kind, l.count, l.sum);
            }
        }
        let mut result = (self.counters.n > 0).then(|| self.counters.value(target.mode));
        let mut means = Acc::default();
        for &(_, count, sum) in &self.histograms {
            if count > 0.0 {
                means.add(sum / count);
            }
        }
        if means.n > 0 {
            result = Some(result.unwrap_or(0.0) + means.value(target.mode));
        }
        if !self.gauges.is_empty() {
            let values = self.gauges.iter().map(|(_, acc)| acc.value(target.mode));
            let combined = match target.combine {
                Combine::Sum => values.sum(),
                Combine::Max => values.fold(f64::NEG_INFINITY, f64::max),
            };
            result = Some(result.unwrap_or(0.0) + combined);
        }
        result
    }
}

pub(super) fn median_gap(times: &mut [i64]) -> Option<f64> {
    times.sort_unstable();
    let mut gaps: Vec<i64> = times
        .windows(2)
        .filter_map(|w| match w {
            [a, b] if b > a => Some(b - a),
            _ => None,
        })
        .collect();
    gaps.sort_unstable();
    let mid = gaps.len() / 2;
    if gaps.len() % 2 == 1 {
        gaps.get(mid).map(|&g| g as f64)
    } else {
        match gaps.get(mid.checked_sub(1)?..=mid) {
            Some([a, b]) => Some((*a as f64 + *b as f64) / 2.0),
            _ => None,
        }
    }
}

impl Reducer {
    pub fn new(target: Target, span: Span) -> Self {
        let buckets = match span {
            Span::Whole => 1,
            Span::Halves(_) => 2,
            Span::Buckets(window) => usize::from(window.buckets.max(1)),
        };
        Reducer {
            target,
            span,
            carry: target.combine == Combine::Sum && matches!(span, Span::Buckets(_)),
            buckets: vec![Bucket::default(); buckets].into_boxed_slice(),
            series: Vec::new(),
        }
    }

    pub fn push(&mut self, record: &MetricRecord<'_>, metric: usize) {
        let index = match self.span {
            Span::Whole => 0,
            Span::Halves(window) => {
                let Some(at) = record.at else { return };
                usize::from(
                    i128::from(at.millis()) * 2
                        >= i128::from(window.from.millis()) + i128::from(window.to.millis()),
                )
            }
            Span::Buckets(window) => {
                let Some(at) = record.at else { return };
                if self.carry && record.kind == MetricKind::Gauge {
                    self.record_time(record.node_id(), metric, at.millis());
                }
                match window.bucket_of(at) {
                    Some(index) => index,
                    None => return,
                }
            }
        };
        let Some(bucket) = self.buckets.get_mut(index) else {
            return;
        };
        if self.target.mode == Mode::Last {
            bucket.keep_latest(Latest::of(record, metric));
        } else {
            bucket.add(
                record.node_id(),
                metric,
                record.kind,
                record.count,
                record.sum,
            );
            if self.carry && record.kind == MetricKind::Gauge {
                bucket.keep_latest(Latest::of(record, metric));
            }
        }
    }

    fn record_time(&mut self, node: u64, metric: usize, at: i64) {
        match self
            .series
            .iter_mut()
            .find(|s| s.node == node && s.metric == metric)
        {
            Some(series) => series.times.push(at),
            None => self.series.push(SeriesTimes {
                node,
                metric,
                times: vec![at],
            }),
        }
    }

    pub fn whole(mut self) -> Option<f64> {
        self.buckets.first_mut()?.reduce(self.target)
    }

    pub fn buckets(mut self) -> impl Iterator<Item = Option<f64>> {
        self.carry_forward();
        let target = self.target;
        self.buckets
            .into_iter()
            .map(move |mut bucket| bucket.reduce(target))
    }

    fn carry_forward(&mut self) {
        let Span::Buckets(window) = self.span else {
            return;
        };
        if !self.carry {
            return;
        }
        let intervals: Vec<(u64, usize, f64)> = self
            .series
            .iter_mut()
            .filter_map(|s| median_gap(&mut s.times).map(|gap| (s.node, s.metric, gap)))
            .collect();
        if intervals.is_empty() {
            return;
        }
        let from = window.from.millis() as f64;
        let width = window.span_ms() as f64 / f64::from(window.buckets.max(1));
        let mut last: Vec<Latest> = Vec::new();
        let mut carried: Vec<(u64, f64)> = Vec::new();
        for (index, bucket) in self.buckets.iter_mut().enumerate() {
            let start = from + width * index as f64;
            let gauges = || bucket.latest.iter().filter(|l| l.kind == MetricKind::Gauge);
            carried.clear();
            carried.extend(
                last.iter()
                    .filter(|prev| {
                        !gauges().any(|l| l.node == prev.node && l.metric == prev.metric)
                            && intervals.iter().any(|&(node, metric, gap)| {
                                node == prev.node
                                    && metric == prev.metric
                                    && prev.at.is_some_and(|at| start - (at as f64) < gap)
                            })
                    })
                    .map(|prev| (prev.node, prev.count)),
            );
            for current in gauges() {
                match last
                    .iter_mut()
                    .find(|l| l.node == current.node && l.metric == current.metric)
                {
                    Some(prev) => *prev = *current,
                    None => last.push(*current),
                }
            }
            for &(node, value) in &carried {
                bucket.add_gauge(node, value);
            }
        }
    }
}
