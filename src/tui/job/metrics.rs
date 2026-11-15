/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::jmap::Client;
use crate::jmap::pages::Fetch;
use crate::tui::exec::{Cancel, JobCx};
use crate::view::metrics::plan::DashboardPlan;
use crate::view::metrics::record::RECORD_PROPERTIES;
use crate::view::metrics::{DashboardData, RangePreset, Window};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};

const METRIC_OBJECT: &str = "x:Metric";
pub const NODE_OBJECT: &str = "x:ClusterNode";
const NODE_PROPERTIES: &[&str] = &["nodeId", "hostname"];

pub enum MetricsJob {
    History {
        client: Arc<Client>,
        plan: Arc<DashboardPlan>,
        cancel: Cancel,
    },
    Nodes {
        client: Arc<Client>,
    },
}

pub enum MetricsOutput {
    History(std::result::Result<Box<DashboardData>, Box<Error>>),
    Nodes(Vec<NodeName>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeName {
    pub id: u64,
    pub hostname: Box<str>,
}

impl MetricsOutput {
    pub fn error(&self) -> Option<&Error> {
        match self {
            MetricsOutput::History(result) => result.as_ref().err().map(AsRef::as_ref),
            MetricsOutput::Nodes(_) => None,
        }
    }
}

impl MetricsJob {
    pub fn run(self, _cx: &JobCx) -> MetricsOutput {
        match self {
            MetricsJob::History {
                client,
                plan,
                cancel,
            } => MetricsOutput::History(history(&client, &plan, &cancel).map_err(Box::new)),
            MetricsJob::Nodes { client } => {
                MetricsOutput::Nodes(nodes(&client).unwrap_or_default())
            }
        }
    }
}

fn nodes(client: &Client) -> Result<Vec<NodeName>> {
    let mut names = Vec::new();
    for page in client.pages(
        NODE_OBJECT,
        None,
        Fetch::Objects {
            properties: Some(NODE_PROPERTIES),
        },
    ) {
        names.extend(page?.objects.iter().filter_map(|object| {
            let id = match object.get("nodeId")? {
                Value::Number(id) => id.as_u64()?,
                Value::String(id) => id.parse().ok()?,
                _ => return None,
            };
            let hostname = object.get("hostname")?.as_str()?.trim();
            (!hostname.is_empty()).then(|| NodeName {
                id,
                hostname: hostname.into(),
            })
        }));
    }
    Ok(names)
}

pub fn node_options(names: &[NodeName], seen: &[u64], selected: Option<u64>) -> Vec<(u64, String)> {
    let mut ids = names
        .iter()
        .map(|name| name.id)
        .chain(seen.iter().copied())
        .chain(selected)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .map(|id| {
            let label = match names.iter().find(|name| name.id == id) {
                Some(name) => format!("{} ({id})", name.hostname),
                None => format!("Node {id}"),
            };
            (id, label)
        })
        .collect()
}

fn history(client: &Client, plan: &DashboardPlan, cancel: &Cancel) -> Result<Box<DashboardData>> {
    let mut agg = plan.history();
    let filter = plan.history_filter();
    for page in client.pages(
        METRIC_OBJECT,
        Some(&filter),
        Fetch::Objects {
            properties: Some(RECORD_PROPERTIES),
        },
    ) {
        if cancel.is_cancelled() {
            break;
        }
        for object in &page?.objects {
            agg.push_object(object);
        }
    }
    Ok(Box::new(agg.finish()))
}

const CACHE_TTL: Duration = Duration::from_secs(60);
const CACHE_ENTRIES: usize = 8;
const MAX_SEEN_NODES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeKey {
    Preset(RangePreset),
    Custom { from: i64, to: i64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheKey {
    pub dashboard: Box<str>,
    pub range: RangeKey,
    pub node: Option<u64>,
}

struct CacheEntry {
    key: CacheKey,
    stored: Instant,
    window: Window,
    data: DashboardData,
}

#[derive(Default)]
pub struct MetricsCache {
    entries: Vec<CacheEntry>,
    nodes: Vec<NodeName>,
    nodes_at: Option<Instant>,
    seen: Vec<u64>,
}

impl MetricsCache {
    pub fn nodes_stale(&self, now: Instant) -> bool {
        self.nodes_at
            .is_none_or(|at| now.saturating_duration_since(at) >= CACHE_TTL)
    }

    pub fn set_nodes(&mut self, nodes: Vec<NodeName>, now: Instant) {
        self.nodes = nodes;
        self.nodes_at = Some(now);
    }

    pub fn node_names(&self) -> &[NodeName] {
        &self.nodes
    }

    pub fn see_nodes(&mut self, ids: &[u64]) {
        for &id in ids {
            if let Err(at) = self.seen.binary_search(&id)
                && self.seen.len() < MAX_SEEN_NODES
            {
                self.seen.insert(at, id);
            }
        }
    }

    pub fn seen_nodes(&self) -> &[u64] {
        &self.seen
    }

    pub fn get(&self, key: &CacheKey, now: Instant) -> Option<(Window, &DashboardData)> {
        self.entries
            .iter()
            .find(|entry| {
                entry.key == *key && now.saturating_duration_since(entry.stored) < CACHE_TTL
            })
            .map(|entry| (entry.window, &entry.data))
    }

    pub fn put(&mut self, key: CacheKey, window: Window, data: DashboardData, now: Instant) {
        self.entries.retain(|entry| {
            entry.key != key && now.saturating_duration_since(entry.stored) < CACHE_TTL
        });
        if self.entries.len() >= CACHE_ENTRIES
            && let Some(oldest) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.stored)
                .map(|(index, _)| index)
        {
            self.entries.swap_remove(oldest);
        }
        self.entries.push(CacheEntry {
            key,
            stored: now,
            window,
            data,
        });
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/job/metrics/tests.rs"]
mod tests;
