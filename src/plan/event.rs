/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Error;
use crate::plan::apply::Summary;
use crate::plan::op::OpKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchAction {
    Create,
    Update,
    Destroy,
}

#[derive(Debug)]
pub enum ApplyEvent<'a> {
    ValueMatchFallback {
        object: &'a str,
    },
    Batch {
        action: BatchAction,
        object: &'a str,
        number: usize,
        of: usize,
        size: usize,
    },
    OpDone {
        index: usize,
        kind: OpKind,
        object: &'a str,
        count: usize,
    },
    ReconcileCleanup {
        index: usize,
        object: &'a str,
        destroyed: usize,
    },
    OpFailed {
        index: usize,
        kind: OpKind,
        object: &'a str,
        error: &'a Error,
    },
    Finished(&'a Summary),
}

#[derive(Debug)]
pub enum SnapshotEvent<'a> {
    ValueMatchFallback {
        object: &'a str,
    },
    AnonymizedSecrets {
        object: &'a str,
    },
    Fetching {
        object: &'a str,
    },
    Fetched {
        count: usize,
    },
    ShardDone {
        object: &'a str,
        variant: Option<&'a str>,
        count: usize,
    },
    FetchingSingleton {
        object: &'a str,
    },
}
