/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use thiserror::Error;

pub type CliResult<T> = Result<T, CliError>;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("no URL provided (use --url or STALWART_URL)")]
    MissingUrl,
    #[error("no credentials provided (use --user / --password or --api-key)")]
    MissingCredentials,
    #[error("no password provided and stdin is not a tty")]
    NoPasswordNoTty,
    #[error("--user and --api-key are mutually exclusive")]
    ConflictingCredentials,

    #[error("authentication failed (HTTP 401)")]
    AuthFailed,
    #[error("permission denied (HTTP 403)")]
    PermissionDenied,
    #[error("not found (HTTP 404)")]
    NotFound,
    #[error("rate limited (HTTP 429)")]
    RateLimited,
    #[error("server returned HTTP {0}")]
    HttpStatus(u16),
    #[error("unexpected server response: {0}")]
    UnexpectedResponse(String),

    #[error("unknown object: {0}")]
    UnknownObject(String),
    #[error("{0} is a view; use the underlying object name")]
    ObjectIsView(String),
    #[error("unknown field: {0}")]
    UnknownField(String),
    #[error("unknown variant '{variant}' for object {object}")]
    UnknownVariant { object: String, variant: String },
    #[error("{0} is not multi-variant; do not use the `/Variant` syntax")]
    NotMultiVariant(String),
    #[error("{0} requires an id")]
    IdRequired(String),
    #[error("singleton id must be 'singleton'")]
    BadSingletonId,

    #[error("jmap error: {type_}: {description}")]
    JmapMethodError { type_: String, description: String },

    #[error("{0}")]
    Message(String),

    #[error(transparent)]
    Network(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl CliError {
    pub fn msg(s: impl Into<String>) -> Self {
        CliError::Message(s.into())
    }
}
