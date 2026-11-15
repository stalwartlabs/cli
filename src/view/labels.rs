/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::Client;
use crate::jmap::client::{CallScope, Request};
use crate::jmap::method::MethodName;
use crate::jmap::response::GetResponse;
use crate::schema::object::FieldSet;
use crate::schema::{FieldType, MapValueType, ScalarType, Schema};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

const MAX_LABELS: usize = 50_000;
const CALL_IDS: [&str; 8] = ["l0", "l1", "l2", "l3", "l4", "l5", "l6", "l7"];

type IdLabels = HashMap<Box<str>, Option<Box<str>>>;

#[derive(Debug, Default)]
pub struct LabelCache {
    by_object: HashMap<Box<str>, IdLabels>,
    len: usize,
}

#[derive(Debug, Default)]
pub struct LabelRequest {
    targets: Vec<LabelTarget>,
}

#[derive(Debug)]
struct LabelTarget {
    object: Box<str>,
    property: Box<str>,
    ids: Vec<Box<str>>,
}

#[derive(Debug, Default)]
pub struct Labels(Vec<ObjectLabels>);

#[derive(Debug)]
struct ObjectLabels {
    object: Box<str>,
    labels: Vec<(Box<str>, Option<Box<str>>)>,
}

#[derive(Serialize)]
struct LabelGet<'a> {
    ids: &'a [Box<str>],
    properties: [&'a str; 1],
}

struct Wanted<'a> {
    schema: &'a Schema,
    ids: HashMap<&'a str, HashSet<&'a str>>,
}

impl LabelCache {
    pub fn get(&self, object: &str, id: &str) -> Option<&str> {
        self.by_object.get(object)?.get(id)?.as_deref()
    }

    pub fn populate<'a>(
        &mut self,
        client: &Client,
        schema: &'a Schema,
        fields: FieldSet<'a>,
        objects: &'a [Map<String, Value>],
    ) {
        let request = self.missing(schema, fields, objects);
        if !request.is_empty() {
            self.insert_all(request.fetch(client, CallScope::Stalwart));
        }
    }

    pub fn missing<'a>(
        &self,
        schema: &'a Schema,
        fields: FieldSet<'a>,
        objects: &'a [Map<String, Value>],
    ) -> LabelRequest {
        let mut wanted = Wanted {
            schema,
            ids: HashMap::new(),
        };
        for obj in objects {
            wanted.object(fields, obj);
        }
        let targets = wanted
            .ids
            .into_iter()
            .filter_map(|(object, ids)| {
                let property = schema.get_object(object)?.label_property()?;
                let ids = self.uncached(object, ids);
                (!ids.is_empty()).then(|| LabelTarget {
                    object: object.into(),
                    property: property.into(),
                    ids,
                })
            })
            .collect();
        LabelRequest { targets }
    }

    pub fn insert_all(&mut self, labels: Labels) {
        let incoming = labels
            .0
            .iter()
            .map(|batch| batch.labels.len())
            .sum::<usize>();
        if self.len.saturating_add(incoming) > MAX_LABELS {
            self.by_object.clear();
            self.len = 0;
        }
        for batch in labels.0 {
            let cached = self.by_object.entry(batch.object).or_default();
            for (id, label) in batch.labels {
                if cached.insert(id, label).is_none() {
                    self.len += 1;
                }
            }
        }
    }

    pub fn insert(&mut self, object: &str, id: &str, label: &str) {
        self.insert_all(Labels(vec![ObjectLabels {
            object: object.into(),
            labels: vec![(id.into(), Some(label.into()))],
        }]));
    }

    fn uncached(&self, object: &str, ids: HashSet<&str>) -> Vec<Box<str>> {
        let cached = self.by_object.get(object);
        ids.into_iter()
            .filter(|id| cached.is_none_or(|labels| !labels.contains_key(*id)))
            .map(Box::from)
            .collect()
    }
}

impl LabelRequest {
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    pub fn fetch(&self, client: &Client, scope: CallScope<'_>) -> Labels {
        let mut out: Vec<ObjectLabels> = self
            .targets
            .iter()
            .map(|target| ObjectLabels {
                object: target.object.clone(),
                labels: Vec::with_capacity(target.ids.len()),
            })
            .collect();
        let max_get = client.max_get();
        let mut calls = self.targets.iter().enumerate().flat_map(|(index, target)| {
            target
                .ids
                .chunks(max_get)
                .map(move |chunk| (index, target, chunk))
        });
        let group_size = CALL_IDS.len().min(client.max_calls());
        let mut group = Vec::with_capacity(group_size);
        loop {
            group.clear();
            group.extend(calls.by_ref().take(group_size));
            if group.is_empty() {
                break;
            }
            let mut request = Request::scoped(scope);
            for ((_, target, chunk), id) in group.iter().zip(CALL_IDS) {
                let args = LabelGet {
                    ids: chunk,
                    properties: [&target.property],
                };
                if request
                    .call(MethodName::get(&target.object), &args, id)
                    .is_err()
                {
                    return Labels(out);
                }
            }
            let Ok(mut response) = client.send(&request) else {
                continue;
            };
            for (&(index, target, chunk), id) in group.iter().zip(CALL_IDS) {
                let Ok(reply) =
                    response.take_as::<GetResponse>(id, MethodName::get(&target.object))
                else {
                    continue;
                };
                let Some(labels) = out.get_mut(index).map(|batch| &mut batch.labels) else {
                    continue;
                };
                let pairs: Vec<(Box<str>, Box<str>)> =
                    label_pairs(&target.property, &reply.list).collect();
                let found: HashSet<&str> = pairs.iter().map(|(id, _)| &**id).collect();
                labels.extend(
                    chunk
                        .iter()
                        .filter(|id| !found.contains(&***id))
                        .map(|id| (id.clone(), None)),
                );
                labels.extend(pairs.into_iter().map(|(id, label)| (id, Some(label))));
            }
        }
        Labels(out)
    }
}

fn label_pairs<'l>(
    property: &'l str,
    list: &'l [Map<String, Value>],
) -> impl Iterator<Item = (Box<str>, Box<str>)> + 'l {
    list.iter().filter_map(move |item| {
        let id = item.get("id").and_then(Value::as_str)?;
        let label: Box<str> = match item.get(property)? {
            Value::String(text) => text.as_str().into(),
            Value::Number(n) => n.to_string().into(),
            Value::Bool(b) => b.to_string().into(),
            _ => return None,
        };
        Some((id.into(), label))
    })
}

impl<'a> Wanted<'a> {
    fn object(&mut self, fields: FieldSet<'a>, obj: &'a Map<String, Value>) {
        for (key, value) in obj {
            if let Some(field) = fields.get(key) {
                self.value(value, &field.typ);
            }
        }
    }

    fn value(&mut self, value: &'a Value, typ: &'a FieldType) {
        match typ {
            FieldType::ObjectId { object_name, .. } => {
                if let Some(id) = value.as_str() {
                    self.ids.entry(object_name).or_default().insert(id);
                }
            }
            FieldType::Set {
                class: ScalarType::ObjectId { object_name },
                ..
            } => {
                if let Some(members) = value.as_object() {
                    self.keys(object_name, members);
                }
            }
            FieldType::Map {
                key_class,
                value_class,
                ..
            } => {
                let Some(map) = value.as_object() else {
                    return;
                };
                if let ScalarType::ObjectId { object_name } = key_class {
                    self.keys(object_name, map);
                }
                if let MapValueType::Object { object_name } = value_class {
                    for item in map.values() {
                        self.nested(object_name, item);
                    }
                }
            }
            FieldType::Object { object_name, .. } => self.nested(object_name, value),
            FieldType::ObjectList { object_name, .. } => {
                if let Some(items) = value.as_object() {
                    for item in items.values() {
                        self.nested(object_name, item);
                    }
                }
            }
            _ => {}
        }
    }

    fn keys(&mut self, object_name: &'a str, map: &'a Map<String, Value>) {
        self.ids
            .entry(object_name)
            .or_default()
            .extend(map.keys().map(String::as_str));
    }

    fn nested(&mut self, object_name: &str, value: &'a Value) {
        let Some(obj) = value.as_object() else {
            return;
        };
        let Some(shape) = self.schema.schemas.get(object_name) else {
            return;
        };
        let schema_name = shape
            .single_name()
            .or_else(|| shape.variant_of(obj)?.schema_name.as_deref());
        if let Some(fields) = schema_name.and_then(|name| self.schema.fields.get(name)) {
            self.object(FieldSet::One(fields), obj);
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/view/labels/tests.rs"]
mod tests;

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/view/labels/test_support.rs"]
mod test_support;
