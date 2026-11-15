/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("invalid JSON: {0}")]
    InvalidJson(serde_json::Error),
    #[error("input must be a JSON object")]
    NotAnObject,
    #[error("field keys for `create` must be top-level properties (got `{0}`)")]
    NestedCreateKey(String),
    #[error("duplicate field key `{0}`")]
    DuplicateField(String),
    #[error("invalid field `{0}`: unterminated quoted key")]
    UnterminatedQuote(String),
    #[error("invalid field `{0}`: empty quoted key")]
    EmptyQuotedKey(String),
    #[error("invalid field `{0}`: expected `=` after quoted key")]
    MissingEqualsAfterQuote(String),
    #[error("invalid field `{0}`: expected KEY=VALUE")]
    MissingEquals(String),
    #[error("invalid JSON in field value: {0}")]
    InvalidFieldJson(serde_json::Error),
    #[error("null is not allowed for non-nullable fields on create")]
    NullNotAllowed,
    #[error("expected boolean, got `{0}`")]
    ExpectedBoolean(String),
    #[error("expected number, got `{0}`")]
    ExpectedNumber(String),
    #[error("expected integer, got `{0}`")]
    ExpectedInteger(String),
    #[error("non-finite number `{0}`")]
    NonFinite(String),
    #[error("field requires a JSON object/array value (wrap in `{{...}}` or `[...]`)")]
    CompositeNeedsJson,
    #[error("invalid filter `{0}`: expected field=value (or field>=value, <=, >, <)")]
    InvalidWhere(String),
    #[error("operator `{0}` is only allowed on number or datetime fields")]
    ComparisonNotAllowed(&'static str),
    #[error("ids array must contain only strings")]
    IdsNotStrings,
}
