/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

mod coerce;
mod error;
pub mod filter;
mod ids;
pub mod patch;

pub use coerce::{BoolWords, Coercer};
pub use error::InputError;
pub use filter::{Where, build_filter};
pub use ids::parse_ids;

use crate::error::{Error, Result};
use crate::schema::FieldType;
use crate::schema::object::FieldSet;
use serde_json::{Map, Value};

const FIELD: Coercer = Coercer::new(BoolWords::Strict);
const TYPE_KEY: &str = "@type";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Create,
    Update,
}

#[derive(Clone, Copy, Debug)]
pub enum Payload<'a> {
    Fields(&'a [String]),
    Json(&'a str),
}

impl Payload<'_> {
    pub fn build(self, fields: FieldSet<'_>, mode: Mode) -> Result<Map<String, Value>> {
        match self {
            Payload::Json(text) => {
                match serde_json::from_str(text.trim()).map_err(InputError::InvalidJson)? {
                    Value::Object(map) => Ok(map),
                    _ => Err(InputError::NotAnObject.into()),
                }
            }
            Payload::Fields(entries) => {
                let mut out = Map::with_capacity(entries.len());
                for raw in entries {
                    let (key, value) = split_kv(raw)?;
                    let (key, ty) = resolve_key(key, fields, mode)?;
                    if mode == Mode::Create && key.contains('/') {
                        return Err(InputError::NestedCreateKey(key).into());
                    }
                    if out.contains_key(&key) {
                        return Err(InputError::DuplicateField(key).into());
                    }
                    let value = FIELD.coerce(value, ty, mode)?;
                    out.insert(key, value);
                }
                Ok(out)
            }
        }
    }
}

fn split_kv(raw: &str) -> Result<(&str, &str), InputError> {
    let quoted = raw
        .strip_prefix('"')
        .map(|rest| (rest, '"'))
        .or_else(|| raw.strip_prefix('\'').map(|rest| (rest, '\'')));
    let Some((rest, quote)) = quoted else {
        return match raw.split_once('=') {
            Some((key, value)) if !key.is_empty() => Ok((key, value)),
            _ => Err(InputError::MissingEquals(raw.to_owned())),
        };
    };
    let (key, after) = rest
        .split_once(quote)
        .ok_or_else(|| InputError::UnterminatedQuote(raw.to_owned()))?;
    if key.is_empty() {
        return Err(InputError::EmptyQuotedKey(raw.to_owned()));
    }
    let value = after
        .strip_prefix('=')
        .ok_or_else(|| InputError::MissingEqualsAfterQuote(raw.to_owned()))?;
    Ok((key, value))
}

fn resolve_key<'a>(
    key: &str,
    fields: FieldSet<'a>,
    mode: Mode,
) -> Result<(String, Option<&'a FieldType>)> {
    if key.eq_ignore_ascii_case(TYPE_KEY) {
        return Ok((TYPE_KEY.to_owned(), None));
    }
    let (head, rest) = match (mode, key.split_once('/')) {
        (Mode::Update, Some((head, rest))) => (head, Some(rest)),
        _ => (key, None),
    };
    let (name, field) = fields
        .get_ci(head)
        .ok_or_else(|| Error::UnknownField(head.to_owned()))?;
    Ok(match rest {
        None => (name.to_owned(), Some(&field.typ)),
        Some(rest) => {
            let mut path = String::with_capacity(name.len() + 1 + rest.len());
            path.push_str(name);
            path.push('/');
            path.push_str(rest);
            (path, None)
        }
    })
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/input/tests.rs"]
mod tests;
