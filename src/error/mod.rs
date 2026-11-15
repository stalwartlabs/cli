/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::plan::{ApplyError, PlanError, SnapshotError};

use crate::auth::AuthError;
use crate::auth::login::BasicReason;
use crate::input::InputError;
use crate::schema::ResolveError;
use reqwest::StatusCode;
use reqwest::header::InvalidHeaderValue;

mod command;

pub use command::CommandError;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no URL provided")]
    MissingUrl,
    #[error("URL '{0}' is missing a scheme; prefix it with https:// or http://")]
    MissingUrlScheme(String),
    #[error("no credentials provided")]
    MissingCredentials,
    #[error("no credentials provided and device sign-in is unavailable")]
    DeviceUnavailable(Box<BasicReason>),
    #[error("no password provided and stdin is not a tty")]
    NoPasswordNoTty,
    #[error("user and API key credentials are mutually exclusive")]
    ConflictingCredentials,

    #[error("authentication failed (HTTP 401)")]
    AuthFailed,
    #[error("permission denied (HTTP 403)")]
    PermissionDenied,
    #[error("not found (HTTP 404)")]
    NotFound,
    #[error("rate limited (HTTP 429)")]
    RateLimited,
    #[error("multi-factor authentication required (HTTP 402)")]
    MfaRequired,
    #[error("session expired; sign in again")]
    SessionExpired,
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("server returned HTTP {0}")]
    HttpStatus(u16),
    #[error("unexpected server response: {0}")]
    UnexpectedResponse(String),

    #[error(
        "could not parse {path} response as JSON ({reason})\n  status: {status}\n  content-type: {content_type}\n  body length: {byte_len} bytes\n  body snippet: {snippet}"
    )]
    SchemaParse {
        path: String,
        reason: String,
        status: u16,
        content_type: String,
        byte_len: usize,
        snippet: String,
    },
    #[error(
        "cached schema is corrupt and was discarded; rerun the command to refetch from the server\n  cache file: {file}\n  reason: {reason}\n  byte length: {byte_len} bytes\n  snippet: {snippet}"
    )]
    SchemaCacheCorrupt {
        file: String,
        reason: String,
        byte_len: usize,
        snippet: String,
        refresh: Box<Error>,
    },
    #[error("could not determine user cache directory")]
    NoCacheDir,
    #[error("invalid auth header: {0}")]
    InvalidAuthHeader(InvalidHeaderValue),

    #[error("unknown field: {0}")]
    UnknownField(String),
    #[error("{0} requires an id")]
    IdRequired(String),
    #[error("singleton id must be 'singleton'")]
    BadSingletonId,

    #[error("jmap error: {type_}: {description}")]
    JmapMethod { type_: String, description: String },

    #[error(transparent)]
    Plan(Box<PlanError>),
    #[error(transparent)]
    Apply(Box<ApplyError>),
    #[error(transparent)]
    Snapshot(Box<SnapshotError>),

    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error(transparent)]
    Input(#[from] InputError),
    #[error(transparent)]
    Command(#[from] CommandError),

    #[error(transparent)]
    Network(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl Error {
    pub fn refresh_failure(&self) -> Option<&Error> {
        match self {
            Error::SchemaCacheCorrupt { refresh, .. } => Some(refresh),
            _ => None,
        }
    }
}

impl From<StatusCode> for Error {
    fn from(status: StatusCode) -> Self {
        match status {
            StatusCode::UNAUTHORIZED => Error::AuthFailed,
            StatusCode::FORBIDDEN => Error::PermissionDenied,
            StatusCode::NOT_FOUND => Error::NotFound,
            StatusCode::TOO_MANY_REQUESTS => Error::RateLimited,
            StatusCode::PAYMENT_REQUIRED => Error::MfaRequired,
            other => Error::HttpStatus(other.as_u16()),
        }
    }
}

impl From<PlanError> for Error {
    fn from(e: PlanError) -> Self {
        Error::Plan(Box::new(e))
    }
}

impl From<ApplyError> for Error {
    fn from(e: ApplyError) -> Self {
        Error::Apply(Box::new(e))
    }
}

impl From<SnapshotError> for Error {
    fn from(e: SnapshotError) -> Self {
        Error::Snapshot(Box::new(e))
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/error/tests.rs"]
mod tests;
