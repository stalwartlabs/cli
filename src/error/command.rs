/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::path::PathBuf;
use std::str::Utf8Error;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{object} {id} not found")]
    NotFound { object: String, id: String },
    #[error("{0} is a singleton and does not support query")]
    SingletonQuery(String),
    #[error("{0} is a singleton and cannot be deleted")]
    SingletonDelete(String),
    #[error("no ids provided")]
    NoIds,
    #[error("id list and standard input are mutually exclusive")]
    ConflictingIds,
    #[error("no ids provided")]
    MissingIds,
    #[error("one or more deletions failed")]
    DeleteFailed,
    #[error("create failed")]
    CreateFailed,
    #[error("update failed")]
    UpdateFailed,
    #[error("no fields to update")]
    EmptyUpdate,
    #[error("{object}: server did not acknowledge update for id `{id}` (the id may not exist)")]
    UpdateNotAcknowledged { object: String, id: String },
    #[error("no object or enum named `{0}`")]
    UnknownDescribeTarget(String),
    #[error("no input provided")]
    NoInput,
    #[error("input sources are mutually exclusive")]
    ConflictingInput,
    #[error("{}: invalid UTF-8: {reason}", path.display())]
    InvalidUtf8 { path: PathBuf, reason: Utf8Error },
    #[error("plan file and standard input are mutually exclusive")]
    ConflictingPlanInput,
    #[error("no plan provided")]
    NoPlan,
}
