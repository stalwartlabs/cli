/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::plan::AT_TYPE;
use crate::plan::format::PlanRef;
use crate::plan::store::Object;
use crate::schema::object::split_view;
use crate::schema::{Field, FieldType, Fields, MapValueType, ObjectSchema, ScalarType, Schema};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Secrets {
    Strip,
    Include,
}

pub fn is_anonymized_secret(v: &Value) -> bool {
    matches!(v, Value::String(s) if !s.is_empty() && s.bytes().all(|b| b == b'*'))
}

pub struct Transformer<'s, 'a> {
    schema: &'s Schema,
    allow: &'a HashSet<&'s str>,
    secrets: Secrets,
    anonymized: bool,
    prefixes: HashMap<&'s str, String>,
}

impl<'s, 'a> Transformer<'s, 'a> {
    pub fn new(schema: &'s Schema, allow: &'a HashSet<&'s str>, secrets: Secrets) -> Self {
        Transformer {
            schema,
            allow,
            secrets,
            anonymized: false,
            prefixes: HashMap::new(),
        }
    }

    pub fn take_anonymized(&mut self) -> bool {
        std::mem::take(&mut self.anonymized)
    }

    pub fn object(&mut self, fields: Option<&'s Fields>, input: &Object) -> Object {
        let mut out = Map::with_capacity(input.len());
        self.fields_into(fields, input, &mut out);
        out
    }

    fn fields_into(&mut self, fields: Option<&'s Fields>, input: &Object, out: &mut Object) {
        if let Some(at_type) = input.get(AT_TYPE) {
            out.insert(AT_TYPE.into(), at_type.clone());
        }
        let Some(fields) = fields else {
            return;
        };
        for (name, field) in &fields.properties {
            self.field_into(name, field, input, out);
        }
    }

    pub fn field_into(
        &mut self,
        name: &String,
        field: &'s Field,
        input: &Object,
        out: &mut Object,
    ) {
        if field.is_server_set() {
            return;
        }
        if field.typ.is_secret_scalar() {
            if input.get(name).is_some_and(is_anonymized_secret) {
                self.anonymized = true;
            }
            if self.secrets == Secrets::Strip {
                return;
            }
        }
        let Some(raw) = input.get(name) else { return };
        if let Some(v) = self.value(&field.typ, raw) {
            out.insert(name.clone(), v);
        }
    }

    pub fn prefix(&mut self, object_name: &'s str) -> &str {
        self.prefixes
            .entry(object_name)
            .or_insert_with(|| PlanRef::prefix(object_name))
    }

    fn reference(&mut self, object_name: &'s str, id: &str) -> String {
        PlanRef {
            prefix: self.prefix(object_name),
            id,
        }
        .reference()
    }

    fn excluded(&self, object_name: &str) -> bool {
        self.allow.contains(split_view(object_name).0)
    }

    fn value(&mut self, t: &'s FieldType, value: &Value) -> Option<Value> {
        if value.is_null() {
            return Some(Value::Null);
        }
        match t {
            FieldType::ObjectId { object_name, .. } => {
                if self.excluded(object_name) {
                    return None;
                }
                match value {
                    Value::String(id) => Some(Value::String(self.reference(object_name, id))),
                    _ => Some(Value::Null),
                }
            }
            FieldType::Set { class, .. } => {
                let Value::Object(map) = value else {
                    return Some(value.clone());
                };
                let ScalarType::ObjectId { object_name } = class else {
                    return Some(value.clone());
                };
                if self.excluded(object_name) {
                    return None;
                }
                let mut out = Map::with_capacity(map.len());
                for (id, v) in map {
                    out.insert(self.reference(object_name, id), v.clone());
                }
                Some(Value::Object(out))
            }
            FieldType::Map {
                key_class,
                value_class,
                ..
            } => {
                let Value::Object(map) = value else {
                    return Some(value.clone());
                };
                let key_object = match key_class {
                    ScalarType::ObjectId { object_name } => {
                        if self.excluded(object_name) {
                            return None;
                        }
                        Some(object_name.as_str())
                    }
                    _ => None,
                };
                let mut out = Map::with_capacity(map.len());
                for (k, v) in map {
                    let key = match key_object {
                        Some(object_name) => self.reference(object_name, k),
                        None => k.clone(),
                    };
                    let v = match value_class {
                        MapValueType::Object { object_name } => match self.embedded(object_name, v)
                        {
                            Some(tv) => tv,
                            None => continue,
                        },
                        _ => v.clone(),
                    };
                    out.insert(key, v);
                }
                Some(Value::Object(out))
            }
            FieldType::Object { object_name, .. } => self.embedded(object_name, value),
            FieldType::ObjectList { object_name, .. } => {
                let Value::Object(map) = value else {
                    return Some(value.clone());
                };
                let mut out = Map::with_capacity(map.len());
                for (idx, item) in map {
                    if let Some(tv) = self.embedded(object_name, item) {
                        out.insert(idx.clone(), tv);
                    }
                }
                Some(Value::Object(out))
            }
            _ => Some(value.clone()),
        }
    }

    fn embedded(&mut self, object_name: &str, value: &Value) -> Option<Value> {
        let Value::Object(map) = value else {
            return Some(value.clone());
        };
        let schema = self.schema;
        let shape = schema.schemas.get(object_name)?;
        let fields = match shape {
            ObjectSchema::Single { schema_name } => schema.fields.get(schema_name)?,
            ObjectSchema::Multiple { .. } => {
                let variant = shape.variant_of(map)?;
                match variant
                    .schema_name
                    .as_ref()
                    .and_then(|n| schema.fields.get(n))
                {
                    Some(f) => f,
                    None => {
                        let mut out = Map::with_capacity(1);
                        out.insert(AT_TYPE.into(), Value::String(variant.name.clone()));
                        return Some(Value::Object(out));
                    }
                }
            }
        };
        let mut out = Map::with_capacity(map.len());
        self.fields_into(Some(fields), map, &mut out);
        Some(Value::Object(out))
    }
}
