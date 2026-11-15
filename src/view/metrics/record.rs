/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::live::{LiveSample, MetricKind};
use crate::view::time::UtcTimestamp;
use serde::Deserialize;
use serde_json::{Map, Value};

pub const RECORD_PROPERTIES: &[&str] = &["@type", "metric", "count", "sum", "nodeId", "timestamp"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetricRecord<'a> {
    pub metric: &'a str,
    pub kind: MetricKind,
    pub count: f64,
    pub sum: f64,
    pub node: Option<u64>,
    pub at: Option<UtcTimestamp>,
}

impl<'a> MetricRecord<'a> {
    pub fn from_object(object: &'a Map<String, Value>) -> Option<Self> {
        let kind = MetricKind::deserialize(object.get("@type")?).ok()?;
        let sum = match (kind, object.get("sum").and_then(Value::as_f64)) {
            (MetricKind::Unknown, _) | (MetricKind::Histogram, None) => return None,
            (_, sum) => sum.unwrap_or(0.0),
        };
        Some(MetricRecord {
            metric: object.get("metric")?.as_str()?,
            kind,
            count: object.get("count")?.as_f64()?,
            sum,
            node: object.get("nodeId").and_then(Value::as_u64),
            at: object
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(UtcTimestamp::parse),
        })
    }

    pub fn from_live(sample: &'a LiveSample) -> Option<Self> {
        let sum = match (sample.kind, sample.sum) {
            (MetricKind::Unknown, _) | (MetricKind::Histogram, None) => return None,
            (_, sum) => sum.unwrap_or(0.0),
        };
        Some(MetricRecord {
            metric: &sample.metric,
            kind: sample.kind,
            count: sample.count,
            sum,
            node: None,
            at: None,
        })
    }

    pub fn node_id(&self) -> u64 {
        self.node.unwrap_or(0)
    }
}
