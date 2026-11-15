/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::plan::AT_TYPE;
use crate::plan::error::{OpRef, PlanError, RefSite};
use crate::plan::format::{PlanRef, RawMatchOn};
use crate::plan::matcher::MatchKey;
use crate::plan::op::{MatchOn, Plan};
use crate::plan::refs::match_ref;
use crate::schema::{Field, ObjectDef};
use serde_json::{Map, Value};
use std::collections::HashSet;

const MATCH_ON_WILDCARD: &str = "*";

impl RawMatchOn {
    pub fn resolve(self, op: OpRef) -> Result<MatchOn, PlanError> {
        match self {
            RawMatchOn::Props(keys) if keys.is_empty() => Err(PlanError::EmptyMatchOn { op }),
            RawMatchOn::Props(keys) => Ok(MatchOn::Props(keys)),
            RawMatchOn::Wildcard(w) if w == MATCH_ON_WILDCARD => Ok(MatchOn::Wildcard),
            RawMatchOn::Wildcard(value) => Err(PlanError::InvalidMatchOn { op, value }),
        }
    }
}

#[derive(Debug)]
pub enum ScopeValue {
    Literal(Value),
    Ref(String),
}

#[derive(Debug)]
pub struct Scope(Vec<(String, ScopeValue)>);

impl Scope {
    pub fn parse(
        def: ObjectDef<'_>,
        scope: Option<Value>,
        value: &Map<String, Value>,
        op: OpRef,
    ) -> Result<Option<Scope>, PlanError> {
        let scope = match scope {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::Object(s)) if s.is_empty() => return Ok(None),
            Some(Value::Object(s)) => s,
            Some(_) => return Err(PlanError::ScopeNotObject { op }),
        };

        let display = def.display_name();
        let fields = def.all_fields();
        for (prop, wanted) in &scope {
            if wanted.is_null() {
                return Err(PlanError::ScopeNull {
                    op,
                    property: prop.clone(),
                });
            }
            if prop == AT_TYPE {
                let declared = def.variants();
                if declared.is_empty() {
                    return Err(PlanError::ScopeNotMultiVariant {
                        op,
                        object: display.to_string(),
                    });
                }
                if !wanted.as_str().is_some_and(|w| def.variant(w).is_some()) {
                    let mut names: Vec<&str> = declared.iter().map(|v| v.name.as_str()).collect();
                    names.sort_unstable();
                    return Err(PlanError::ScopeUnknownVariant {
                        op,
                        wanted: Box::new(wanted.clone()),
                        object: display.to_string(),
                        expected: names.join(", "),
                    });
                }
                continue;
            }
            if fields.defs(prop).any(Field::is_server_set) {
                return Err(PlanError::ScopeServerSet {
                    op,
                    property: prop.clone(),
                });
            }
            if !fields.is_empty() && fields.get(prop).is_none() {
                return Err(PlanError::ScopeUnknownProperty {
                    op,
                    property: prop.clone(),
                    object: display.to_string(),
                });
            }
        }

        for (client_id, body) in value {
            let Some(body) = body.as_object() else {
                continue;
            };
            for (prop, wanted) in &scope {
                if let Some(actual) = body.get(prop)
                    && actual != wanted
                    && PlanRef::client_id_of(actual).is_none()
                        == PlanRef::client_id_of(wanted).is_none()
                {
                    return Err(PlanError::EntryOutsideScope {
                        op,
                        client_id: client_id.clone(),
                        property: prop.clone(),
                        actual: Box::new(actual.clone()),
                        wanted: Box::new(wanted.clone()),
                    });
                }
            }
        }

        Ok(Some(Scope(
            scope
                .into_iter()
                .map(|(property, raw)| {
                    let reference = PlanRef::client_id_of(&raw)
                        .filter(|_| fields.defs(&property).any(Field::is_object_id))
                        .map(str::to_string);
                    let value = match reference {
                        Some(client_id) => ScopeValue::Ref(client_id),
                        None => ScopeValue::Literal(raw),
                    };
                    (property, value)
                })
                .collect(),
        )))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ScopeValue)> {
        self.0
            .iter()
            .map(|(property, value)| (property.as_str(), value))
    }

    fn refs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.iter().filter_map(|(property, value)| match value {
            ScopeValue::Ref(client_id) => Some((property, client_id.as_str())),
            ScopeValue::Literal(_) => None,
        })
    }
}

impl Plan<'_> {
    pub fn validate_references(&self) -> Result<()> {
        let mut declared: HashSet<&str> = HashSet::new();
        for op in &self.ops {
            if let Some(scope) = op.merge().and_then(|m| m.scope.as_ref()) {
                for (prop, client_id) in scope.refs() {
                    if !declared.contains(client_id) {
                        return Err(
                            PlanError::unresolved(op.def, RefSite::Scope, prop, client_id).into(),
                        );
                    }
                }
            }
            let Some(value) = op.declared() else {
                continue;
            };
            declared.extend(value.keys().map(String::as_str));
            let Some(merge) = op.merge() else {
                continue;
            };
            let MatchKey::Props(props) = MatchKey::resolve(op.def, merge.match_on.as_ref()) else {
                continue;
            };
            for body in value.values().filter_map(Value::as_object) {
                let fields = op.def.fields_of(body);
                for p in props.iter() {
                    if let Some(client_id) = body.get(p).and_then(|raw| match_ref(fields, p, raw))
                        && !declared.contains(client_id)
                    {
                        return Err(
                            PlanError::unresolved(op.def, RefSite::Match, p, client_id).into()
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
