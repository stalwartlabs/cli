/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::view::ViewDef;
use crate::schema::{AT_TYPE, FieldType, Fields, ObjectSchema, ObjectVariant, Schema};
use serde_json::{Map, Value};

const ID: &str = "id";
const SECRET_MASK: &str = "****";

impl Schema {
    pub fn create_defaults(
        &self,
        view: &ViewDef<'_>,
        variant: Option<&ObjectVariant>,
    ) -> Map<String, Value> {
        let object = view.object.name();
        let mut defaults = Map::new();
        let mut active = None;
        match view.object.shape {
            Some(ObjectSchema::Single { schema_name }) => {
                active = self.fields.get(schema_name);
                merge_defaults(&mut defaults, active);
            }
            Some(ObjectSchema::Multiple { .. }) => {
                if let Some(variant) = variant.or(view.variant) {
                    active = variant
                        .schema_name
                        .as_deref()
                        .and_then(|name| self.fields.get(name));
                    merge_defaults(&mut defaults, active);
                    defaults.insert(AT_TYPE.to_owned(), Value::from(variant.name.as_str()));
                }
            }
            None => {}
        }
        self.nested_defaults(active, &mut defaults);
        if let Some(statics) = view.static_filter {
            let parent = match view.object.shape {
                Some(ObjectSchema::Multiple { .. }) => self.fields.get(object),
                _ => None,
            };
            for (key, value) in statics {
                let known = key == AT_TYPE
                    || [active, parent]
                        .into_iter()
                        .flatten()
                        .any(|fields| fields.properties.contains_key(key));
                if known {
                    merge_entry(&mut defaults, key, value);
                }
            }
        }
        defaults
    }

    pub fn new_object_value(&self, schema_name: &str, variant: Option<&str>) -> Map<String, Value> {
        let mut value = self.embedded_with(schema_name, Map::new(), variant);
        let chosen = value
            .get(AT_TYPE)
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.complete_struct(schema_name, chosen.as_deref().or(variant), &mut value);
        value
    }

    pub fn switch_variant(
        &self,
        current: &Map<String, Value>,
        object_or_schema: &str,
        to: &ObjectVariant,
    ) -> Map<String, Value> {
        let defaults = self.embedded_with(object_or_schema, Map::new(), Some(&to.name));
        let from = current
            .get(AT_TYPE)
            .and_then(Value::as_str)
            .and_then(|name| self.variant_fields(object_or_schema, Some(name)));
        let target = self.variant_fields(object_or_schema, Some(&to.name));
        carry_over_shared(from, target, current, defaults)
    }

    fn variant_fields(&self, schema_name: &str, variant: Option<&str>) -> Option<&Fields> {
        match self.schemas.get(schema_name)? {
            ObjectSchema::Single { schema_name } => self.fields.get(schema_name),
            ObjectSchema::Multiple { variants } => {
                let variant = match variant {
                    Some(name) => variants.iter().find(|v| v.name == name)?,
                    None => variants.first()?,
                };
                self.fields.get(variant.schema_name.as_deref()?)
            }
        }
    }

    fn embedded_with(
        &self,
        schema_name: &str,
        overrides: Map<String, Value>,
        variant: Option<&str>,
    ) -> Map<String, Value> {
        let Some(shape) = self.schemas.get(schema_name) else {
            return overrides;
        };
        let mut result = Map::new();
        let fields = match shape {
            ObjectSchema::Single { schema_name } => self.fields.get(schema_name),
            ObjectSchema::Multiple { variants } => {
                let chosen = match variant {
                    Some(name) => variants.iter().find(|v| v.name == name),
                    None => variants.first(),
                };
                let Some(chosen) = chosen else {
                    let mut result = Map::new();
                    result.insert(AT_TYPE.to_owned(), Value::from(variant.unwrap_or_default()));
                    merge_map(&mut result, &overrides);
                    return result;
                };
                result.insert(AT_TYPE.to_owned(), Value::from(chosen.name.as_str()));
                chosen
                    .schema_name
                    .as_deref()
                    .and_then(|name| self.fields.get(name))
            }
        };
        merge_defaults(&mut result, fields);
        self.nested_defaults(fields, &mut result);
        merge_map(&mut result, &overrides);
        result
    }

    fn nested_defaults(&self, fields: Option<&Fields>, target: &mut Map<String, Value>) {
        for (name, field) in fields.into_iter().flat_map(|f| &f.properties) {
            let FieldType::Object { object_name, .. } = &field.typ else {
                continue;
            };
            let overrides = match target.get_mut(name) {
                Some(Value::Null) => continue,
                Some(Value::Object(map)) => std::mem::take(map),
                _ => Map::new(),
            };
            let variant = match self.schemas.get(object_name) {
                Some(ObjectSchema::Multiple { variants }) => overrides
                    .get(AT_TYPE)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| variants.first().map(|v| v.name.clone())),
                _ => None,
            };
            let nested = self.embedded_with(object_name, overrides, variant.as_deref());
            if !nested.is_empty() {
                target.insert(name.clone(), Value::Object(nested));
            }
        }
    }

    fn complete_struct(
        &self,
        schema_name: &str,
        variant: Option<&str>,
        target: &mut Map<String, Value>,
    ) {
        let Some(fields) = self.variant_fields(schema_name, variant) else {
            return;
        };
        for (name, field) in &fields.properties {
            if field.is_server_set() {
                continue;
            }
            match &field.typ {
                FieldType::Boolean => {
                    target.entry(name.clone()).or_insert(Value::Bool(false));
                }
                FieldType::Object {
                    object_name,
                    nullable: false,
                } => {
                    let mut nested = match target.get_mut(name) {
                        None => Map::new(),
                        Some(Value::Object(map)) => std::mem::take(map),
                        Some(_) => continue,
                    };
                    let nested_variant = match self.schemas.get(object_name) {
                        Some(ObjectSchema::Multiple { variants }) => nested
                            .get(AT_TYPE)
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .or_else(|| variants.first().map(|v| v.name.clone())),
                        _ => None,
                    };
                    self.complete_struct(object_name, nested_variant.as_deref(), &mut nested);
                    if !nested.is_empty() || target.contains_key(name) {
                        target.insert(name.clone(), Value::Object(nested));
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn carry_over_shared(
    from: Option<&Fields>,
    to: Option<&Fields>,
    current: &Map<String, Value>,
    mut defaults: Map<String, Value>,
) -> Map<String, Value> {
    let (Some(from), Some(to)) = (from, to) else {
        return defaults;
    };
    for (name, field) in &to.properties {
        if name == AT_TYPE || name == ID || field.is_server_set() {
            continue;
        }
        let (Some(previous), Some(value)) = (from.properties.get(name), current.get(name)) else {
            continue;
        };
        if contains_secret_mask(value) || previous.typ != field.typ {
            continue;
        }
        defaults.insert(name.clone(), value.clone());
    }
    defaults
}

pub fn deep_merge(target: &mut Value, source: &Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => merge_map(target, source),
        (target, source) => *target = source.clone(),
    }
}

fn merge_map(target: &mut Map<String, Value>, source: &Map<String, Value>) {
    for (key, value) in source {
        merge_entry(target, key, value);
    }
}

fn merge_entry(target: &mut Map<String, Value>, key: &str, value: &Value) {
    match target.get_mut(key) {
        Some(existing) => deep_merge(existing, value),
        None => {
            target.insert(key.to_owned(), value.clone());
        }
    }
}

fn merge_defaults(target: &mut Map<String, Value>, fields: Option<&Fields>) {
    for (key, value) in fields.into_iter().flat_map(|f| &f.defaults) {
        merge_entry(target, key, value);
    }
}

fn contains_secret_mask(value: &Value) -> bool {
    match value {
        Value::String(text) => text == SECRET_MASK,
        Value::Array(items) => items.iter().any(contains_secret_mask),
        Value::Object(map) => map.values().any(contains_secret_mask),
        _ => false,
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/schema/defaults/tests.rs"]
mod tests;
