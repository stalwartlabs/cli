/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::error::CliResult;
use crate::jmap::Jmap;
use crate::schema::{FieldType, Fields, MapValueType, ObjectSchema, ScalarType, Schema};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct DisplayCache {
    by_object: HashMap<String, HashMap<String, String>>,
}

impl DisplayCache {
    pub fn new() -> Self {
        DisplayCache::default()
    }

    pub fn get(&self, object_name: &str, id: &str) -> Option<&str> {
        self.by_object
            .get(object_name)?
            .get(id)
            .map(String::as_str)
    }

    fn filter_uncached(&self, object_name: &str, ids: HashSet<String>) -> Vec<String> {
        match self.by_object.get(object_name) {
            None => ids.into_iter().collect(),
            Some(cached) => ids.into_iter().filter(|id| !cached.contains_key(id)).collect(),
        }
    }

    pub fn populate_from_objects(
        &mut self,
        jmap: &Jmap,
        schema: &Schema,
        fields: &Fields,
        values: &[&Map<String, Value>],
    ) -> CliResult<()> {
        let mut needed: HashMap<String, HashSet<String>> = HashMap::new();
        for v in values {
            collect_map(schema, fields, v, &mut needed);
        }
        self.populate(jmap, schema, needed)
    }

    pub fn populate(
        &mut self,
        jmap: &Jmap,
        schema: &Schema,
        needed: HashMap<String, HashSet<String>>,
    ) -> CliResult<()> {
        for (object_name, ids) in needed {
            let Some(list) = schema.lists.get(&object_name) else {
                continue;
            };
            let Some(label_prop) = &list.label_property else {
                continue;
            };
            let ids_vec: Vec<Value> = self
                .filter_uncached(&object_name, ids)
                .into_iter()
                .map(Value::String)
                .collect();
            if ids_vec.is_empty() {
                continue;
            }
            let args = serde_json::json!({
                "ids": ids_vec,
                "properties": [label_prop.as_str()],
            });
            let method = format!("{object_name}/get");
            let result = match jmap.call(&method, args) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let Some(list_val) = result.get("list").and_then(Value::as_array) else {
                continue;
            };
            for item in list_val {
                let Some(obj) = item.as_object() else {
                    continue;
                };
                let Some(id) = obj.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let Some(raw) = obj.get(label_prop) else {
                    continue;
                };
                let label = match raw {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => continue,
                };
                self.by_object
                    .entry(object_name.clone())
                    .or_default()
                    .insert(id.to_string(), label);
            }
        }
        Ok(())
    }
}

fn collect_map(
    schema: &Schema,
    fields: &Fields,
    value: &Map<String, Value>,
    out: &mut HashMap<String, HashSet<String>>,
) {
    for (key, v) in value {
        let Some(field) = fields.properties.get(key) else {
            continue;
        };
        collect_value(schema, v, &field.typ, out);
    }
}

fn collect_value(
    schema: &Schema,
    value: &Value,
    field_type: &FieldType,
    out: &mut HashMap<String, HashSet<String>>,
) {
    match field_type {
        FieldType::ObjectId { object_name, .. } => {
            if let Some(id) = value.as_str() {
                out.entry(object_name.clone())
                    .or_default()
                    .insert(id.to_string());
            }
        }
        FieldType::Set { class, .. } => {
            if let ScalarType::ObjectId { object_name } = class
                && let Some(map) = value.as_object()
            {
                let entry = out.entry(object_name.clone()).or_default();
                for k in map.keys() {
                    entry.insert(k.clone());
                }
            }
        }
        FieldType::Map {
            key_class,
            value_class,
            ..
        } => {
            let Some(map) = value.as_object() else { return };
            if let ScalarType::ObjectId { object_name } = key_class {
                let entry = out.entry(object_name.clone()).or_default();
                for k in map.keys() {
                    entry.insert(k.clone());
                }
            }
            if let MapValueType::Object { object_name } = value_class {
                for v in map.values() {
                    collect_nested(schema, v, object_name, out);
                }
            }
        }
        FieldType::Object { object_name, .. } => {
            collect_nested(schema, value, object_name, out);
        }
        FieldType::ObjectList { object_name, .. } => {
            let Some(map) = value.as_object() else { return };
            for v in map.values() {
                collect_nested(schema, v, object_name, out);
            }
        }
        _ => {}
    }
}

fn collect_nested(
    schema: &Schema,
    value: &Value,
    object_name: &str,
    out: &mut HashMap<String, HashSet<String>>,
) {
    let Some(obj) = value.as_object() else { return };
    let Some(obj_schema) = schema.schemas.get(object_name) else {
        return;
    };
    let fields = match obj_schema {
        ObjectSchema::Single { schema_name } => schema.fields.get(schema_name),
        ObjectSchema::Multiple { variants } => {
            let Some(at_type) = obj.get("@type").and_then(Value::as_str) else {
                return;
            };
            let Some(variant) = variants.iter().find(|v| v.name == at_type) else {
                return;
            };
            let Some(schema_name) = &variant.schema_name else {
                return;
            };
            schema.fields.get(schema_name)
        }
    };
    if let Some(f) = fields {
        collect_map(schema, f, obj, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_with(entries: &[(&str, &str, &str)]) -> DisplayCache {
        let mut c = DisplayCache::new();
        for (obj, id, label) in entries {
            c.by_object
                .entry((*obj).to_string())
                .or_default()
                .insert((*id).to_string(), (*label).to_string());
        }
        c
    }

    #[test]
    fn get_returns_label_when_present() {
        let c = cache_with(&[("x:Domain", "d1", "example.com")]);
        assert_eq!(c.get("x:Domain", "d1"), Some("example.com"));
    }

    #[test]
    fn get_returns_none_for_unknown_object() {
        let c = cache_with(&[("x:Domain", "d1", "example.com")]);
        assert_eq!(c.get("x:Account", "d1"), None);
    }

    #[test]
    fn get_returns_none_for_unknown_id() {
        let c = cache_with(&[("x:Domain", "d1", "example.com")]);
        assert_eq!(c.get("x:Domain", "d2"), None);
    }

    #[test]
    fn get_does_not_mutate_or_allocate_into_cache() {
        let c = cache_with(&[("x:Domain", "d1", "example.com")]);
        let before = c.by_object.get("x:Domain").map(|m| m.len()).unwrap_or(0);
        let _ = c.get("x:Domain", "missing");
        let _ = c.get("x:Account", "anything");
        let after = c.by_object.get("x:Domain").map(|m| m.len()).unwrap_or(0);
        assert_eq!(before, after);
        assert!(!c.by_object.contains_key("x:Account"));
    }

    fn ids(strs: &[&str]) -> HashSet<String> {
        strs.iter().map(|s| (*s).to_string()).collect()
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    #[test]
    fn filter_uncached_returns_all_when_object_unknown() {
        let c = DisplayCache::new();
        let result = c.filter_uncached("x:Domain", ids(&["a", "b", "c"]));
        assert_eq!(sorted(result), vec!["a", "b", "c"]);
    }

    #[test]
    fn filter_uncached_drops_already_cached_ids() {
        let c = cache_with(&[("x:Domain", "a", "alpha"), ("x:Domain", "c", "gamma")]);
        let result = c.filter_uncached("x:Domain", ids(&["a", "b", "c", "d"]));
        assert_eq!(sorted(result), vec!["b", "d"]);
    }

    #[test]
    fn filter_uncached_returns_empty_when_all_cached() {
        let c = cache_with(&[("x:Domain", "a", "alpha"), ("x:Domain", "b", "beta")]);
        let result = c.filter_uncached("x:Domain", ids(&["a", "b"]));
        assert!(result.is_empty());
    }

    #[test]
    fn filter_uncached_isolates_per_object_type() {
        let c = cache_with(&[("x:Domain", "shared", "d1")]);
        let result = c.filter_uncached("x:Account", ids(&["shared"]));
        assert_eq!(sorted(result), vec!["shared"]);
    }
}
