/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod apply;
pub mod bulk;
pub mod error;
pub mod event;
pub mod exec;
pub mod format;
pub mod matcher;
pub mod op;
pub mod refs;
pub mod snapshot;
pub mod store;
pub mod validate;

use crate::schema::AT_TYPE;

const ID: &str = "id";

pub use apply::{ApplyOptions, OnError, Summary};
pub use error::{ApplyError, PlanError, SnapshotError};
pub use event::{ApplyEvent, BatchAction, SnapshotEvent};
pub use op::{OpKind, Plan, PlanStats};
pub use snapshot::{Secrets, Snapshot, SnapshotStats};

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/plan/tests/mod.rs"]
mod tests;
