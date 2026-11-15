/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::jmap::Client;
use crate::jmap::client::CallScope;
use crate::jmap::pages::Fetch;
use crate::jmap::response::SetFailures;
use crate::jmap::set_error::SetError;
use crate::plan::exec::{Batch, batches};
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};
use std::ops::ControlFlow;

pub const MAX_KEPT_FAILURES: usize = 1000;

#[derive(Debug, Clone, Copy)]
pub enum BulkTarget<'a> {
    Ids(&'a [String]),
    Matching(Option<&'a Value>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkEvent {
    Resolving {
        fetched: usize,
    },
    Resolved {
        total: usize,
    },
    Batch {
        batch: Batch,
        done: usize,
        failed: usize,
    },
}

#[derive(Debug)]
pub enum BulkStatus {
    Complete,
    Cancelled,
    Failed(Box<Error>),
}

#[derive(Debug)]
pub struct BulkSummary {
    pub status: BulkStatus,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub failures: Vec<(String, SetError)>,
}

#[derive(Serialize)]
struct DestroyArgs<'a> {
    destroy: &'a [String],
}

#[derive(Serialize)]
struct UpdateArgs<'a> {
    update: SamePatch<'a>,
}

struct SamePatch<'a> {
    ids: &'a [String],
    patch: &'a Map<String, Value>,
}

impl Serialize for SamePatch<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.ids.len()))?;
        for id in self.ids {
            map.serialize_entry(id, self.patch)?;
        }
        map.end()
    }
}

#[derive(Clone, Copy)]
enum Change<'a> {
    Destroy,
    Patch(&'a Map<String, Value>),
}

pub fn destroy(
    client: &Client,
    scope: CallScope<'_>,
    object: &str,
    target: BulkTarget<'_>,
    on: &mut dyn FnMut(BulkEvent) -> ControlFlow<()>,
) -> BulkSummary {
    run(client, scope, object, target, Change::Destroy, on)
}

pub fn set_property(
    client: &Client,
    scope: CallScope<'_>,
    object: &str,
    target: BulkTarget<'_>,
    patch: &Map<String, Value>,
    on: &mut dyn FnMut(BulkEvent) -> ControlFlow<()>,
) -> BulkSummary {
    run(client, scope, object, target, Change::Patch(patch), on)
}

fn run(
    client: &Client,
    scope: CallScope<'_>,
    object: &str,
    target: BulkTarget<'_>,
    change: Change<'_>,
    on: &mut dyn FnMut(BulkEvent) -> ControlFlow<()>,
) -> BulkSummary {
    let mut summary = BulkSummary {
        status: BulkStatus::Cancelled,
        total: 0,
        done: 0,
        failed: 0,
        failures: Vec::new(),
    };
    if let Err(err) = apply(client, scope, object, target, change, on, &mut summary) {
        summary.status = BulkStatus::Failed(Box::new(err));
    }
    summary
}

fn apply(
    client: &Client,
    scope: CallScope<'_>,
    object: &str,
    target: BulkTarget<'_>,
    change: Change<'_>,
    on: &mut dyn FnMut(BulkEvent) -> ControlFlow<()>,
    summary: &mut BulkSummary,
) -> Result<()> {
    let fetched;
    let ids = match target {
        BulkTarget::Ids(ids) => ids,
        BulkTarget::Matching(filter) => {
            let mut all = Vec::new();
            for page in client.pages_in(scope, object, filter, Fetch::Ids) {
                all.extend(page?.ids);
                if on(BulkEvent::Resolving { fetched: all.len() }).is_break() {
                    return Ok(());
                }
            }
            fetched = all;
            &fetched
        }
    };
    summary.total = ids.len();
    if on(BulkEvent::Resolved { total: ids.len() }).is_break() {
        return Ok(());
    }
    for (batch, chunk) in batches(ids, client.max_set()) {
        let event = BulkEvent::Batch {
            batch,
            done: summary.done,
            failed: summary.failed,
        };
        if on(event).is_break() {
            return Ok(());
        }
        let failures = match change {
            Change::Destroy => {
                let response = client.set_in(scope, object, &DestroyArgs { destroy: chunk })?;
                summary.done += response.destroyed.len();
                response.not_destroyed
            }
            Change::Patch(patch) => {
                let args = UpdateArgs {
                    update: SamePatch { ids: chunk, patch },
                };
                let response = client.set_in(scope, object, &args)?;
                summary.done += response.updated.len();
                response.not_updated
            }
        };
        summary.keep(failures);
    }
    summary.status = BulkStatus::Complete;
    Ok(())
}

impl BulkSummary {
    pub fn error(&self) -> Option<&Error> {
        match &self.status {
            BulkStatus::Failed(err) => Some(err),
            BulkStatus::Complete | BulkStatus::Cancelled => None,
        }
    }

    fn keep(&mut self, failures: SetFailures) {
        for failure in failures {
            self.failed += 1;
            if self.failures.len() < MAX_KEPT_FAILURES {
                self.failures.push(failure);
            }
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/plan/bulk/tests.rs"]
mod tests;
