/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::plan::error::{PlanError, RefSite};
use crate::plan::format::PlanRef;
use crate::plan::store::Object;
use crate::plan::validate::{Scope, ScopeValue};
use crate::schema::{Field, Fields, ObjectDef};
use crate::view::RefLabels;
use serde::{Serialize, Serializer};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

pub fn values_match(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None | Some(Value::Null), None | Some(Value::Null)) => true,
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedScope<'p>(Vec<(&'p str, Cow<'p, Value>)>);

impl<'p> FromIterator<(&'p str, Cow<'p, Value>)> for ResolvedScope<'p> {
    fn from_iter<I: IntoIterator<Item = (&'p str, Cow<'p, Value>)>>(iter: I) -> Self {
        ResolvedScope(iter.into_iter().collect())
    }
}

impl ResolvedScope<'_> {
    pub fn contains(&self, candidate: &Object) -> bool {
        self.0
            .iter()
            .all(|(p, wanted)| values_match(candidate.get(*p), Some(wanted)))
    }

    pub fn fill<'a>(&self, body: &'a Object) -> Cow<'a, Object> {
        if self.0.iter().all(|(p, _)| body.contains_key(*p)) {
            return Cow::Borrowed(body);
        }
        let mut merged = body.clone();
        for (p, wanted) in &self.0 {
            if !merged.contains_key(*p) {
                merged.insert((*p).to_string(), wanted.as_ref().clone());
            }
        }
        Cow::Owned(merged)
    }
}

pub fn match_ref<'v>(fields: Option<&Fields>, property: &str, raw: &'v Value) -> Option<&'v str> {
    let is_ref = fields
        .and_then(|f| f.properties.get(property))
        .is_some_and(Field::is_object_id);
    PlanRef::client_id_of(raw).filter(|_| is_ref)
}

#[derive(Debug, Default)]
pub struct ClientIdRegistry {
    ids: HashMap<String, String>,
}

impl ClientIdRegistry {
    pub fn get(&self, client_id: &str) -> Option<&str> {
        self.ids.get(client_id).map(String::as_str)
    }

    pub fn insert(&mut self, client_id: String, server_id: String) {
        self.ids.insert(client_id, server_id);
    }

    pub fn merge(&mut self, created: Option<&HashMap<String, String>>) {
        for (client_id, server_id) in created.into_iter().flatten() {
            if self.get(client_id) != Some(server_id.as_str()) {
                self.ids.insert(client_id.clone(), server_id.clone());
            }
        }
    }

    pub fn resolve_ref<'r>(
        &'r self,
        def: ObjectDef<'_>,
        site: RefSite,
        property: &str,
        client_id: &str,
    ) -> Result<&'r str, PlanError> {
        self.get(client_id)
            .ok_or_else(|| PlanError::unresolved(def, site, property, client_id))
    }

    pub fn resolve_match_value<'v>(
        &self,
        def: ObjectDef<'_>,
        fields: Option<&Fields>,
        property: &str,
        raw: &'v Value,
    ) -> Result<Cow<'v, Value>, PlanError> {
        match match_ref(fields, property, raw) {
            Some(client_id) => self
                .resolve_ref(def, RefSite::Match, property, client_id)
                .map(|server_id| Cow::Owned(Value::String(server_id.to_string()))),
            None => Ok(Cow::Borrowed(raw)),
        }
    }

    pub fn resolve_scope<'p>(
        &self,
        def: ObjectDef<'_>,
        scope: &'p Scope,
    ) -> Result<ResolvedScope<'p>, PlanError> {
        scope
            .iter()
            .map(|(property, value)| {
                let value = match value {
                    ScopeValue::Literal(raw) => Cow::Borrowed(raw),
                    ScopeValue::Ref(client_id) => Cow::Owned(Value::String(
                        self.resolve_ref(def, RefSite::Scope, property, client_id)?
                            .to_string(),
                    )),
                };
                Ok((property, value))
            })
            .collect()
    }

    pub fn request_ids<'r>(
        &'r self,
        refs: &'r HashSet<&'r str>,
        in_request: &'r HashSet<&'r str>,
    ) -> RequestIds<'r> {
        RequestIds {
            ids: &self.ids,
            refs,
            in_request,
        }
    }

    pub fn labels(&self, refs: &HashSet<&str>) -> RefLabels {
        RefLabels::from_pairs(
            refs.iter()
                .filter_map(|r| self.ids.get_key_value(*r))
                .map(|(client_id, server_id)| (server_id.as_str(), client_id.as_str())),
        )
    }
}

pub struct RequestIds<'r> {
    ids: &'r HashMap<String, String>,
    refs: &'r HashSet<&'r str>,
    in_request: &'r HashSet<&'r str>,
}

impl Serialize for RequestIds<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(
            self.refs
                .iter()
                .filter(|r| !self.in_request.contains(*r))
                .filter_map(|r| self.ids.get_key_value(*r)),
        )
    }
}

pub fn collect_refs<'v>(value: &'v Value, out: &mut HashSet<&'v str>) {
    match value {
        Value::String(s) => {
            if let Some(id) = PlanRef::client_id(s) {
                out.insert(id);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_refs(v, out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                if let Some(id) = PlanRef::client_id(k) {
                    out.insert(id);
                }
                collect_refs(v, out);
            }
        }
        _ => {}
    }
}
