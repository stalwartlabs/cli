/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::jmap::Client;
use crate::jmap::client::Request;
use crate::jmap::method::MethodName;
use crate::jmap::pages::Fetch;
use crate::jmap::response::SetResponse;
use crate::plan::error::{ApplyError, OpRef, SetAction, SetFailure};
use crate::plan::format::PlanRef;
use crate::plan::refs::{ClientIdRegistry, collect_refs};
use crate::plan::store::{Object, object_id};
use crate::schema::object::display_name;
use crate::view::RefLabels;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;

const SET_CALL_ID: &str = "c0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batch {
    pub number: usize,
    pub of: usize,
    pub size: usize,
}

pub fn batches<T>(items: &[T], size: usize) -> impl Iterator<Item = (Batch, &[T])> {
    let size = size.max(1);
    let of = items.len().div_ceil(size);
    items.chunks(size).enumerate().map(move |(n, chunk)| {
        (
            Batch {
                number: n + 1,
                of,
                size: chunk.len(),
            },
            chunk,
        )
    })
}

#[derive(Serialize)]
struct DestroyArgs<'a> {
    destroy: &'a [String],
}

#[derive(Serialize)]
struct CreateArgs<'a> {
    create: Entries<'a>,
}

#[derive(Serialize)]
struct UpdateArgs<'a> {
    update: Entries<'a>,
}

struct Entries<'a>(&'a [(&'a str, &'a Value)]);

impl Serialize for Entries<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

pub fn destroy_chunk(client: &Client, object: &str, chunk: &[String]) -> Result<SetResponse> {
    client.set(object, &DestroyArgs { destroy: chunk })
}

pub struct Created<'v> {
    body: Option<&'v Object>,
    assigned: Object,
}

impl Created<'_> {
    pub fn id(&self) -> Option<&str> {
        object_id(&self.assigned)
    }

    pub fn into_object(self) -> Object {
        let mut obj = self.body.cloned().unwrap_or_default();
        obj.extend(self.assigned);
        obj
    }
}

pub struct Exec<'c> {
    client: &'c Client,
    pub ids: ClientIdRegistry,
}

impl<'c> Exec<'c> {
    pub fn new(client: &'c Client) -> Self {
        Exec {
            client,
            ids: ClientIdRegistry::default(),
        }
    }

    pub fn client(&self) -> &'c Client {
        self.client
    }

    pub fn fetch_ids(&self, object: &str, filter: Option<&Value>) -> Result<Vec<String>> {
        let mut all = Vec::new();
        for page in self.client.pages(object, filter, Fetch::Ids) {
            all.extend(page?.ids);
        }
        Ok(all)
    }

    pub fn destroy(
        &self,
        object: &str,
        ids: &[String],
        on_batch: &mut dyn FnMut(Batch) -> ControlFlow<()>,
    ) -> (usize, Result<()>) {
        let mut destroyed = 0usize;
        for (batch, chunk) in batches(ids, self.client.max_set()) {
            if on_batch(batch).is_break() {
                return (destroyed, Ok(()));
            }
            let result = match destroy_chunk(self.client, object, chunk) {
                Ok(result) => result,
                Err(e) => return (destroyed, Err(e)),
            };
            destroyed += result.destroyed.len();
            if let Some((id, err)) = result.not_destroyed.first() {
                let failure =
                    SetFailure::new(object, id, SetAction::Destroy, err, RefLabels::default());
                return (destroyed, Err(failure.into()));
            }
        }
        (destroyed, Ok(()))
    }

    fn send_set(
        &mut self,
        object: &str,
        args: &impl Serialize,
        refs: &HashSet<&str>,
        in_request: &HashSet<&str>,
    ) -> Result<SetResponse> {
        let method = MethodName::set(object);
        let mut request = Request::new();
        request.call(method, args, SET_CALL_ID)?;
        if !refs.is_empty() {
            request.created_ids(&self.ids.request_ids(refs, in_request))?;
        }
        let mut response = self.client.send(&request)?;
        self.ids.merge(response.created_ids());
        response.take_as(SET_CALL_ID, method)
    }

    pub fn create<'v>(
        &mut self,
        object: &str,
        value: &'v Map<String, Value>,
        op: OpRef,
        on_batch: &mut dyn FnMut(Batch) -> ControlFlow<()>,
    ) -> (Vec<Created<'v>>, Result<()>) {
        let entries: Vec<(&str, &Value)> = value.iter().map(|(k, v)| (k.as_str(), v)).collect();
        let mut created_objects: Vec<Created<'v>> = Vec::with_capacity(entries.len());
        let mut in_request: HashSet<&str> = HashSet::new();
        let mut refs: HashSet<&str> = HashSet::new();

        for (batch, chunk) in batches(&entries, self.client.max_set()) {
            in_request.clear();
            in_request.extend(chunk.iter().map(|(k, _)| *k));
            refs.clear();
            for (_, v) in chunk {
                collect_refs(v, &mut refs);
            }
            if on_batch(batch).is_break() {
                break;
            }

            let result = match self.send_set(
                object,
                &CreateArgs {
                    create: Entries(chunk),
                },
                &refs,
                &in_request,
            ) {
                Ok(result) => result,
                Err(e) => return (created_objects, Err(e)),
            };

            let failure = result.not_created.first().map(|(bad_id, err)| {
                let labels = self.ids.labels(&refs);
                SetFailure::new(object, bad_id, SetAction::Create(op), err, labels).into()
            });

            for (client_id, assigned) in result.created {
                let Value::Object(assigned) = assigned else {
                    continue;
                };
                let Some(server_id) = object_id(&assigned) else {
                    continue;
                };
                let body = value.get(&client_id).and_then(Value::as_object);
                if self.ids.get(&client_id) != Some(server_id) {
                    self.ids.insert(client_id, server_id.to_string());
                }
                created_objects.push(Created { body, assigned });
            }
            if let Some(failure) = failure {
                return (created_objects, Err(failure));
            }
        }
        (created_objects, Ok(()))
    }

    pub fn update_batch(
        &mut self,
        object: &str,
        chunk: &[(&str, &Value)],
    ) -> (HashMap<String, Option<Value>>, Result<()>) {
        let mut refs: HashSet<&str> = HashSet::new();
        for (_, value) in chunk {
            collect_refs(value, &mut refs);
        }
        let result = match self.send_set(
            object,
            &UpdateArgs {
                update: Entries(chunk),
            },
            &refs,
            &HashSet::new(),
        ) {
            Ok(result) => result,
            Err(e) => return (HashMap::new(), Err(e)),
        };
        let outcome = if let Some((bad_id, err)) = result.not_updated.first() {
            let labels = self.ids.labels(&refs);
            Err(SetFailure::new(object, bad_id, SetAction::Update, err, labels).into())
        } else if let Some((id, _)) = chunk
            .iter()
            .find(|(id, _)| !result.updated.contains_key(*id))
        {
            Err(ApplyError::NotUpdated {
                object: display_name(object).to_string(),
                id: id.to_string(),
            }
            .into())
        } else {
            Ok(())
        };
        (result.updated, outcome)
    }

    pub fn update(&mut self, object: &str, id: &str, value: &Value) -> Result<(usize, String)> {
        let real_id = match PlanRef::client_id(id) {
            Some(client_id) => self.ids.get(client_id).map(str::to_string).ok_or_else(|| {
                ApplyError::UnknownUpdateRef {
                    object: display_name(object).to_string(),
                    client_id: client_id.to_string(),
                }
            })?,
            None => id.to_string(),
        };

        let mut refs: HashSet<&str> = HashSet::new();
        collect_refs(value, &mut refs);
        let result = self.send_set(
            object,
            &UpdateArgs {
                update: Entries(&[(real_id.as_str(), value)]),
            },
            &refs,
            &HashSet::new(),
        )?;

        if let Some((bad_id, err)) = result.not_updated.first() {
            let labels = self.ids.labels(&refs);
            return Err(SetFailure::new(object, bad_id, SetAction::Update, err, labels).into());
        }
        let updated = result.updated.len();
        if updated == 0 {
            return Err(ApplyError::NotUpdated {
                object: display_name(object).to_string(),
                id: real_id,
            }
            .into());
        }
        Ok((updated, real_id))
    }
}
