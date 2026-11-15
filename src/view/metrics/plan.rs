/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use super::record::MetricRecord;
use super::reduce::{Reducer, Span, Target};
use super::{
    ChartData, DashboardData, Delta, ItemData, RankEntry, SPARKLINE_BUCKETS, StatData, Window,
};
use crate::jmap::live::{LiveSample, MetricKind};
use crate::schema::{CardSource, Dashboard, ItemKind, Series};
use serde_json::{Map, Value};
use std::mem;

#[derive(Clone, Debug)]
pub struct DashboardPlan {
    window: Window,
    node: Option<u64>,
    metrics: Box<[Box<str>]>,
    routes: Box<[Vec<usize>]>,
    reducers: Vec<Reducer>,
    items: Box<[ItemSpec]>,
    live: LiveAgg,
}

#[derive(Clone, Copy, Debug)]
enum ItemSpec {
    Stat { sparkline: bool, delta: bool },
    Idle,
    Chart(usize),
    Headline,
    Ranking(usize),
    Unknown,
}

#[derive(Debug)]
pub struct HistoryAgg {
    plan: DashboardPlan,
    nodes: Vec<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct LiveAgg {
    metrics: Box<[Box<str>]>,
    latest: Box<[Option<LiveValue>]>,
    cards: Box<[LiveCard]>,
}

#[derive(Clone, Copy, Debug)]
struct LiveValue {
    kind: MetricKind,
    count: f64,
    sum: f64,
}

#[derive(Clone, Debug)]
struct LiveCard {
    item: usize,
    target: Target,
    metrics: Box<[usize]>,
}

struct Builder<'a> {
    metrics: &'a [&'a str],
    routes: Vec<Vec<usize>>,
    reducers: Vec<Reducer>,
}

impl DashboardPlan {
    pub fn new(dashboard: &Dashboard, window: Window, node: Option<u64>) -> Self {
        let items = dashboard.items();
        let mut history: Vec<&str> = Vec::new();
        let mut live: Vec<&str> = Vec::new();
        for item in items {
            match &item.kind {
                ItemKind::Stat(card) if card.source == CardSource::Live => {
                    live.extend(card.metrics.iter().map(String::as_str))
                }
                ItemKind::Stat(card) if card.source == CardSource::History => {
                    history.extend(card.metrics.iter().map(String::as_str))
                }
                ItemKind::Chart(chart) => history.extend(series_metrics(&chart.series)),
                ItemKind::Ranking(ranking) => history.extend(series_metrics(&ranking.series)),
                ItemKind::Headline(headline) => history.extend(
                    headline
                        .numerator
                        .iter()
                        .chain(&headline.denominator)
                        .map(String::as_str),
                ),
                ItemKind::Stat(_) | ItemKind::Unknown => {}
            }
        }
        history.sort_unstable();
        history.dedup();
        live.sort_unstable();
        live.dedup();

        let mut builder = Builder {
            metrics: &history,
            routes: vec![Vec::new(); history.len()],
            reducers: Vec::new(),
        };
        let mut live_cards = Vec::new();
        let specs = items
            .iter()
            .enumerate()
            .map(|(index, item)| match &item.kind {
                ItemKind::Stat(card) if card.source == CardSource::Live => {
                    live_cards.push(LiveCard {
                        item: index,
                        target: Target::new(card.aggregate, card.scope),
                        metrics: indices(&live, &card.metrics).collect(),
                    });
                    ItemSpec::Idle
                }
                ItemKind::Stat(card) if card.source == CardSource::History => {
                    let target = Target::new(card.aggregate, card.scope);
                    builder.add(&card.metrics, target, Span::Whole);
                    if card.sparkline {
                        let sparkline = Span::Buckets(window.with_buckets(SPARKLINE_BUCKETS));
                        builder.add(&card.metrics, target, sparkline);
                    }
                    if card.delta {
                        builder.add(&card.metrics, target, Span::Halves(window));
                    }
                    ItemSpec::Stat {
                        sparkline: card.sparkline,
                        delta: card.delta,
                    }
                }
                ItemKind::Stat(_) => ItemSpec::Idle,
                ItemKind::Chart(chart) => {
                    builder.add_series(&chart.series, Span::Buckets(window));
                    ItemSpec::Chart(chart.series.len())
                }
                ItemKind::Ranking(ranking) => {
                    builder.add_series(&ranking.series, Span::Whole);
                    ItemSpec::Ranking(ranking.series.len())
                }
                ItemKind::Headline(headline) => {
                    let target = Target::new(headline.aggregate, headline.scope);
                    builder.add(&headline.numerator, target, Span::Whole);
                    builder.add(&headline.denominator, target, Span::Whole);
                    ItemSpec::Headline
                }
                ItemKind::Unknown => ItemSpec::Unknown,
            })
            .collect();

        let Builder {
            routes, reducers, ..
        } = builder;
        DashboardPlan {
            window,
            node,
            metrics: history.iter().map(|&m| Box::from(m)).collect(),
            routes: routes.into_boxed_slice(),
            reducers,
            items: specs,
            live: LiveAgg {
                latest: vec![None; live.len()].into_boxed_slice(),
                metrics: live.iter().map(|&m| Box::from(m)).collect(),
                cards: live_cards.into_boxed_slice(),
            },
        }
    }

    pub fn has_history(&self) -> bool {
        !self.metrics.is_empty()
    }

    pub fn history_filter(&self) -> Value {
        let mut filter = Map::with_capacity(4);
        let mut iso = String::new();
        let _ = self.window.from.format_iso(&mut iso);
        filter.insert(
            "timestampIsGreaterThanOrEqual".into(),
            Value::String(mem::take(&mut iso)),
        );
        let _ = self.window.to.format_iso(&mut iso);
        filter.insert("timestampIsLessThanOrEqual".into(), Value::String(iso));
        filter.insert(
            "metric".into(),
            self.metrics
                .iter()
                .map(|m| Value::String(m.to_string()))
                .collect(),
        );
        if let Some(node) = self.node {
            filter.insert("nodeId".into(), Value::from(node));
        }
        Value::Object(filter)
    }

    pub fn live_metrics(&self) -> impl Iterator<Item = &str> {
        self.live.metrics.iter().map(AsRef::as_ref)
    }

    pub fn live(&self) -> LiveAgg {
        self.live.clone()
    }

    pub fn history(&self) -> HistoryAgg {
        HistoryAgg {
            plan: self.clone(),
            nodes: Vec::new(),
        }
    }
}

impl Builder<'_> {
    fn add(&mut self, metrics: &[String], target: Target, span: Span) {
        let id = self.reducers.len();
        self.reducers.push(Reducer::new(target, span));
        for index in indices(self.metrics, metrics) {
            if let Some(route) = self.routes.get_mut(index)
                && route.last() != Some(&id)
            {
                route.push(id);
            }
        }
    }

    fn add_series(&mut self, series: &[Series], span: Span) {
        for s in series {
            self.add(&s.metrics, Target::new(s.aggregate, s.scope), span);
        }
    }
}

fn series_metrics(series: &[Series]) -> impl Iterator<Item = &str> {
    series
        .iter()
        .flat_map(|s| s.metrics.iter().map(String::as_str))
}

fn indices<'a>(table: &'a [&str], names: &'a [String]) -> impl Iterator<Item = usize> + 'a {
    names
        .iter()
        .filter_map(|name| table.binary_search(&name.as_str()).ok())
}

impl HistoryAgg {
    pub fn push(&mut self, record: &MetricRecord<'_>) {
        if let Some(node) = record.node
            && let Err(at) = self.nodes.binary_search(&node)
        {
            self.nodes.insert(at, node);
        }
        let plan = &mut self.plan;
        let Ok(metric) = plan
            .metrics
            .binary_search_by(|m| m.as_ref().cmp(record.metric))
        else {
            return;
        };
        let Some(route) = plan.routes.get(metric) else {
            return;
        };
        for &id in route {
            if let Some(reducer) = plan.reducers.get_mut(id) {
                reducer.push(record, metric);
            }
        }
    }

    pub fn push_object(&mut self, object: &Map<String, Value>) {
        if let Some(record) = MetricRecord::from_object(object) {
            self.push(&record);
        }
    }

    pub fn finish(self) -> DashboardData {
        let mut reducers = self.plan.reducers.into_iter();
        let items = self
            .plan
            .items
            .iter()
            .map(|spec| match *spec {
                ItemSpec::Stat { sparkline, delta } => {
                    let value = reducers.next().and_then(Reducer::whole).unwrap_or(0.0);
                    let sparkline = sparkline
                        .then(|| reducers.next())
                        .flatten()
                        .map(|r| r.buckets().map(|v| v.unwrap_or(0.0)).collect());
                    let delta = delta.then(|| reducers.next()).flatten().and_then(|r| {
                        let mut halves = r.buckets().map(|v| v.unwrap_or(0.0));
                        Delta::between(halves.next()?, halves.next()?)
                    });
                    ItemData::Stat(StatData {
                        value,
                        sparkline,
                        delta,
                    })
                }
                ItemSpec::Idle => ItemData::Stat(StatData::default()),
                ItemSpec::Chart(series) => ItemData::Chart(ChartData {
                    series: reducers
                        .by_ref()
                        .take(series)
                        .map(|r| r.buckets().collect())
                        .collect(),
                }),
                ItemSpec::Headline => {
                    let numerator = reducers.next().and_then(Reducer::whole).unwrap_or(0.0);
                    let denominator = reducers.next().and_then(Reducer::whole).unwrap_or(0.0);
                    ItemData::Headline {
                        ratio: (denominator > 0.0).then(|| numerator / denominator),
                    }
                }
                ItemSpec::Ranking(series) => {
                    let mut entries: Vec<RankEntry> = reducers
                        .by_ref()
                        .take(series)
                        .enumerate()
                        .map(|(series, r)| RankEntry {
                            series,
                            value: r.whole().unwrap_or(0.0),
                        })
                        .collect();
                    entries.sort_by(|a, b| b.value.total_cmp(&a.value));
                    ItemData::Ranking(entries)
                }
                ItemSpec::Unknown => ItemData::Unknown,
            })
            .collect();
        DashboardData {
            items,
            nodes: self.nodes,
        }
    }
}

impl LiveAgg {
    pub fn apply(&mut self, frame: &[LiveSample]) {
        for sample in frame {
            if let Ok(index) = self
                .metrics
                .binary_search_by(|m| m.as_ref().cmp(&sample.metric))
                && let Some(slot) = self.latest.get_mut(index)
                && let Some(record) = MetricRecord::from_live(sample)
            {
                *slot = Some(LiveValue {
                    kind: record.kind,
                    count: record.count,
                    sum: record.sum,
                });
            }
        }
    }

    pub fn fill(&self, data: &mut DashboardData) {
        for card in &self.cards {
            let Some(ItemData::Stat(stat)) = data.items.get_mut(card.item) else {
                continue;
            };
            let mut reducer = Reducer::new(card.target, Span::Whole);
            for &metric in &card.metrics {
                if let (Some(Some(value)), Some(name)) =
                    (self.latest.get(metric), self.metrics.get(metric))
                {
                    reducer.push(
                        &MetricRecord {
                            metric: name,
                            kind: value.kind,
                            count: value.count,
                            sum: value.sum,
                            node: None,
                            at: None,
                        },
                        metric,
                    );
                }
            }
            stat.value = reducer.whole().unwrap_or(0.0);
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/view/metrics/plan/tests.rs"]
mod tests;
