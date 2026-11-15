/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::app::Cx;
use crate::tui::job::objects::{ObjectsJob, ObjectsKind};
use crate::tui::msg::Job;

pub mod action;
pub mod bootstrap;
pub mod browser;
pub mod dashboard;
pub mod delivery;
pub mod detail;
pub mod form;
pub mod log_view;
pub mod login;
pub mod message;
pub mod tools;
pub mod trace_detail;
pub mod tracing;

pub fn objects_job(cx: &Cx<'_>, kind: ObjectsKind) -> Job {
    Job::Objects(ObjectsJob::new(
        cx.conn.client.clone(),
        cx.conn.schema.clone(),
        cx.conn.active_account.clone(),
        kind,
    ))
}
