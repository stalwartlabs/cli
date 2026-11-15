/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::defaults::deep_merge;
use crate::schema::object::FieldSet;
use crate::schema::{AT_TYPE, Field};
use serde_json::{Map, Value};
use std::collections::HashMap;

pub const SECRET_MASK: &str = "****";
const ID: &str = "id";

pub fn jmap_patch(
    original: &Map<String, Value>,
    modified: &Map<String, Value>,
) -> Map<String, Value> {
    let mut patch = Map::new();
    let variant_changed = match (original.get(AT_TYPE), modified.get(AT_TYPE)) {
        (Some(before), Some(after)) => before != after,
        _ => false,
    };
    let mut path = String::new();
    for (key, before) in original {
        if key == ID {
            continue;
        }
        match modified.get(key) {
            Some(after) => {
                path.clear();
                escape_pointer_token(key, &mut path);
                diff_values(&mut path, before, after, &mut patch);
            }
            None if variant_changed => {}
            None => {
                path.clear();
                escape_pointer_token(key, &mut path);
                patch.insert(path.clone(), Value::Null);
            }
        }
    }
    for (key, after) in modified {
        if key != ID && !original.contains_key(key) {
            path.clear();
            escape_pointer_token(key, &mut path);
            patch.insert(path.clone(), after.clone());
        }
    }
    patch
}

pub fn update_patch(
    original: &Map<String, Value>,
    edited: &Map<String, Value>,
    fields: FieldSet<'_>,
) -> Map<String, Value> {
    let mut patch = Map::new();
    if original.get(AT_TYPE) != edited.get(AT_TYPE) {
        for (key, value) in edited {
            if key == ID {
                continue;
            }
            if let Some(field) = fields.get(key)
                && (field.is_server_set() || is_unchanged_secret(field, value))
            {
                continue;
            }
            patch.insert(key.clone(), without_masked_secrets(value));
        }
        return patch;
    }
    for (key, value) in jmap_patch(original, edited) {
        let head = key.split_once('/').map_or(key.as_str(), |(head, _)| head);
        if let Some(field) = fields.get(head)
            && (!field.is_patchable() || is_unchanged_secret(field, &value))
        {
            continue;
        }
        if key.contains('/') && value.as_str() == Some(SECRET_MASK) {
            continue;
        }
        let value = without_masked_secrets(&value);
        patch.insert(key, value);
    }
    patch
}

pub fn create_payload(
    edited: &Map<String, Value>,
    fields: FieldSet<'_>,
    static_filter: Option<&HashMap<String, Value>>,
) -> Map<String, Value> {
    let mut payload = Map::new();
    for (key, value) in static_filter.into_iter().flatten() {
        if key == AT_TYPE || fields.get(key).is_some() {
            payload.insert(key.clone(), value.clone());
        }
    }
    if let Some(variant) = edited.get(AT_TYPE) {
        payload.insert(AT_TYPE.to_owned(), variant.clone());
    }
    for (name, field) in fields.all_defs() {
        let Some(value) = edited.get(name) else {
            continue;
        };
        if field.is_server_set() || is_unchanged_secret(field, value) {
            continue;
        }
        match payload.get_mut(name) {
            Some(existing) => deep_merge(existing, value),
            None => {
                payload.insert(name.to_owned(), value.clone());
            }
        }
    }
    payload
}

pub fn escape_pointer_token(token: &str, out: &mut String) {
    for c in token.chars() {
        match c {
            '~' => out.push_str("~0"),
            '/' => out.push_str("~1"),
            c => out.push(c),
        }
    }
}

fn diff_values(path: &mut String, before: &Value, after: &Value, patch: &mut Map<String, Value>) {
    match (before, after) {
        (Value::Object(before), Value::Object(after)) => {
            if before.get(AT_TYPE) != after.get(AT_TYPE) {
                patch.insert(path.clone(), Value::Object(after.clone()));
                return;
            }
            let base = path.len();
            for (key, old) in before {
                path.truncate(base);
                path.push('/');
                escape_pointer_token(key, path);
                match after.get(key) {
                    Some(new) => diff_values(path, old, new, patch),
                    None => {
                        patch.insert(path.clone(), Value::Null);
                    }
                }
            }
            for (key, new) in after {
                if !before.contains_key(key) {
                    path.truncate(base);
                    path.push('/');
                    escape_pointer_token(key, path);
                    patch.insert(path.clone(), new.clone());
                }
            }
            path.truncate(base);
        }
        _ if before != after => {
            patch.insert(path.clone(), after.clone());
        }
        _ => {}
    }
}

fn is_unchanged_secret(field: &Field, value: &Value) -> bool {
    field.typ.is_secret_scalar() && value.as_str() == Some(SECRET_MASK)
}

fn without_masked_secrets(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(without_masked_secrets).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, nested)| nested.as_str() != Some(SECRET_MASK))
                .map(|(key, nested)| (key.clone(), without_masked_secrets(nested)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/input/patch/tests.rs"]
mod tests;
