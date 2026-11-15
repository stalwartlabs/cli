/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use serde::{Deserialize, Serialize};

const FULL_ROW_SPAN: u8 = 12;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(from = "RawDashboard")]
pub struct Dashboard {
    pub id: String,
    pub label: String,
    items: Vec<DashboardItem>,
}

#[derive(Deserialize)]
struct RawDashboard {
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    items: Vec<DashboardItem>,
    #[serde(default)]
    cards: Vec<Card>,
    #[serde(default)]
    charts: Vec<Chart>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DashboardItem {
    #[serde(flatten)]
    pub placement: Placement,
    #[serde(flatten)]
    pub kind: ItemKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ItemKind {
    Stat(Card),
    Chart(Chart),
    Headline(Headline),
    Ranking(Ranking),
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub source: CardSource,
    #[serde(default)]
    pub metrics: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<Aggregate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<MetricScope>,
    #[serde(default)]
    pub format: MetricFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub sparkline: bool,
    #[serde(default)]
    pub delta: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polarity: Option<Polarity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Chart {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub kind: ChartKind,
    #[serde(default)]
    pub series: Vec<Series>,
    #[serde(default)]
    pub stacked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_format: Option<MetricFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub metrics: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<Aggregate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<MetricScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Headline {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub numerator: Vec<String>,
    #[serde(default)]
    pub denominator: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<Aggregate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<MetricScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Ranking {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub series: Vec<Series>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_format: Option<MetricFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Tone {
    Flat,
    Accent,
    Ink,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum CardSource {
    Live,
    #[default]
    History,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Aggregate {
    Sum,
    Avg,
    Last,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MetricScope {
    Node,
    Cluster,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum MetricFormat {
    #[default]
    Number,
    Bytes,
    Duration,
    Percent,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum ChartKind {
    #[default]
    Line,
    Area,
    Bar,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Polarity {
    HigherIsBetter,
    LowerIsBetter,
    Neutral,
    #[serde(other)]
    Unknown,
}

impl Dashboard {
    pub fn items(&self) -> &[DashboardItem] {
        &self.items
    }
}

impl From<RawDashboard> for Dashboard {
    fn from(raw: RawDashboard) -> Self {
        let items = if raw.items.is_empty() {
            let mut items = Vec::with_capacity(raw.cards.len() + raw.charts.len());
            items.extend(
                raw.cards
                    .into_iter()
                    .map(ItemKind::Stat)
                    .map(DashboardItem::new),
            );
            let odd_last = (raw.charts.len() % 2 == 1).then(|| raw.charts.len() - 1);
            items.extend(raw.charts.into_iter().enumerate().map(|(index, chart)| {
                let mut item = DashboardItem::new(ItemKind::Chart(chart));
                if odd_last == Some(index) {
                    item.placement.span = Some(FULL_ROW_SPAN);
                }
                item
            }));
            items
        } else {
            raw.items
        };
        Dashboard {
            id: raw.id,
            label: raw.label,
            items,
        }
    }
}

impl DashboardItem {
    fn new(kind: ItemKind) -> Self {
        DashboardItem {
            placement: Placement::default(),
            kind,
        }
    }
}
