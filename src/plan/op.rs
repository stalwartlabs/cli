/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::plan::error::{OpRef, PlanError};
use crate::plan::format::{RawMatchOn, RawOp, parse_ndjson};
use crate::plan::matcher::MatchKey;
use crate::plan::validate::Scope;
use crate::schema::object::{IdError, ObjectId};
use crate::schema::{ObjectDef, Schema};
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Create,
    Update,
    Destroy,
    Upsert,
    Reconcile,
}

impl OpKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            OpKind::Create => "create",
            OpKind::Update => "update",
            OpKind::Destroy => "destroy",
            OpKind::Upsert => "upsert",
            OpKind::Reconcile => "reconcile",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            OpKind::Create => "Create",
            OpKind::Update => "Update",
            OpKind::Destroy => "Destroy",
            OpKind::Upsert => "Upsert",
            OpKind::Reconcile => "Reconcile",
        }
    }

    pub const fn past_tense(self) -> &'static str {
        match self {
            OpKind::Create => "Created",
            OpKind::Update => "Updated",
            OpKind::Destroy => "Destroyed",
            OpKind::Upsert => "Upserted",
            OpKind::Reconcile => "Reconciled",
        }
    }
}

#[derive(Debug, Clone)]
pub enum MatchOn {
    Wildcard,
    Props(Vec<String>),
}

#[derive(Debug)]
pub struct Merge {
    pub match_on: Option<MatchOn>,
    pub scope: Option<Scope>,
    pub value: Map<String, Value>,
}

#[derive(Debug)]
pub enum Action {
    Create(Map<String, Value>),
    Update { id: String, value: Value },
    Destroy { filter: Option<Value> },
    Upsert(Merge),
    Reconcile(Merge),
}

#[derive(Debug)]
pub struct Op<'s> {
    pub index: usize,
    pub def: ObjectDef<'s>,
    pub action: Action,
}

impl<'s> Op<'s> {
    pub fn object(&self) -> &'s str {
        self.def.name()
    }

    pub fn op_ref(&self) -> OpRef {
        OpRef {
            index: self.index,
            kind: self.kind(),
        }
    }

    pub fn kind(&self) -> OpKind {
        match self.action {
            Action::Create(_) => OpKind::Create,
            Action::Update { .. } => OpKind::Update,
            Action::Destroy { .. } => OpKind::Destroy,
            Action::Upsert(_) => OpKind::Upsert,
            Action::Reconcile(_) => OpKind::Reconcile,
        }
    }

    pub fn merge(&self) -> Option<&Merge> {
        match &self.action {
            Action::Upsert(m) | Action::Reconcile(m) => Some(m),
            _ => None,
        }
    }

    pub fn declared(&self) -> Option<&Map<String, Value>> {
        match &self.action {
            Action::Create(value) => Some(value),
            Action::Upsert(m) | Action::Reconcile(m) => Some(&m.value),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PlanStats {
    pub destroys: usize,
    pub updates: usize,
    pub creates: usize,
    pub create_objects: usize,
    pub upserts: usize,
    pub upsert_objects: usize,
    pub reconciles: usize,
    pub reconcile_objects: usize,
}

impl PlanStats {
    pub fn objects(&self) -> usize {
        self.create_objects + self.upsert_objects + self.reconcile_objects
    }

    fn count(&mut self, action: &Action) {
        match action {
            Action::Create(value) => {
                self.creates += 1;
                self.create_objects += value.len();
            }
            Action::Update { .. } => self.updates += 1,
            Action::Destroy { .. } => self.destroys += 1,
            Action::Upsert(m) => {
                self.upserts += 1;
                self.upsert_objects += m.value.len();
            }
            Action::Reconcile(m) => {
                self.reconciles += 1;
                self.reconcile_objects += m.value.len();
            }
        }
    }
}

pub struct Plan<'s> {
    pub(crate) ops: Vec<Op<'s>>,
    stats: PlanStats,
}

impl<'s> Plan<'s> {
    pub fn parse(schema: &'s Schema, ndjson: &str) -> Result<Plan<'s>> {
        let raw = parse_ndjson(ndjson)?;
        let plan = Plan::resolve(schema, raw)?;
        plan.validate_references()?;
        Ok(plan)
    }

    pub(crate) fn resolve(schema: &'s Schema, raw: Vec<RawOp>) -> Result<Plan<'s>> {
        let mut ops = Vec::with_capacity(raw.len());
        let mut stats = PlanStats::default();
        for (index, raw) in raw.into_iter().enumerate() {
            let op = Op::resolve(schema, index, raw)?;
            stats.count(&op.action);
            ops.push(op);
        }
        Ok(Plan { ops, stats })
    }

    pub fn stats(&self) -> &PlanStats {
        &self.stats
    }
}

impl<'s> Op<'s> {
    fn resolve(schema: &'s Schema, index: usize, raw: RawOp) -> Result<Op<'s>> {
        let op = OpRef {
            index,
            kind: raw.kind(),
        };
        let def = schema.object(raw.object())?;
        let action = match raw {
            RawOp::Update {
                id, scope, value, ..
            } => {
                op.reject_scope(scope.as_ref())?;
                let id = op.update_id(def, id.as_deref())?;
                Action::Update { id, value }
            }
            RawOp::Destroy { scope, value, .. } => {
                op.reject_scope(scope.as_ref())?;
                op.reject_singleton(def)?;
                Action::Destroy {
                    filter: value.filter(|v| !v.is_null()),
                }
            }
            RawOp::Create { scope, value, .. } => {
                op.reject_scope(scope.as_ref())?;
                op.reject_empty(&value)?;
                Action::Create(op.normalise_ids(value)?)
            }
            RawOp::Upsert {
                match_on,
                scope,
                value,
                ..
            } => {
                op.reject_singleton(def)?;
                op.reject_empty(&value)?;
                Action::Upsert(Merge::resolve(def, match_on, scope, value, op)?)
            }
            RawOp::Reconcile {
                match_on,
                scope,
                value,
                ..
            } => {
                op.reject_singleton(def)?;
                Action::Reconcile(Merge::resolve(def, match_on, scope, value, op)?)
            }
        };
        Ok(Op { index, def, action })
    }
}

impl Merge {
    fn resolve(
        def: ObjectDef<'_>,
        match_on: Option<RawMatchOn>,
        scope: Option<Value>,
        value: Map<String, Value>,
        op: OpRef,
    ) -> Result<Merge> {
        let match_on = match_on.map(|m| m.resolve(op)).transpose()?;
        if op.kind == OpKind::Reconcile
            && match_on.is_none()
            && matches!(MatchKey::resolve(def, None), MatchKey::Value)
        {
            return Err(PlanError::NoMatchKey {
                op,
                object: def.display_name().to_string(),
            }
            .into());
        }
        let scope = Scope::parse(def, scope, &value, op)?;
        Ok(Merge {
            match_on,
            scope,
            value: op.normalise_ids(value)?,
        })
    }
}

impl OpRef {
    fn reject_scope(self, scope: Option<&Value>) -> Result<(), PlanError> {
        match scope {
            None | Some(Value::Null) => Ok(()),
            Some(_) => Err(PlanError::ScopeNotAllowed { op: self }),
        }
    }

    fn reject_singleton(self, def: ObjectDef<'_>) -> Result<(), PlanError> {
        if def.is_singleton() {
            return Err(PlanError::Singleton {
                op: self,
                object: def.display_name().to_string(),
            });
        }
        Ok(())
    }

    fn reject_empty(self, value: &Map<String, Value>) -> Result<(), PlanError> {
        if value.is_empty() {
            return Err(PlanError::EmptyValue { op: self });
        }
        Ok(())
    }

    fn normalise_ids(self, value: Map<String, Value>) -> Result<Map<String, Value>, PlanError> {
        let mut normalised = Map::with_capacity(value.len());
        for (key, body) in value {
            let key = match key.strip_prefix('#') {
                Some(stripped) => stripped.to_string(),
                None => key,
            };
            if normalised.contains_key(&key) {
                return Err(PlanError::DuplicateId { op: self, id: key });
            }
            normalised.insert(key, body);
        }
        Ok(normalised)
    }

    fn update_id(self, def: ObjectDef<'_>, id: Option<&str>) -> Result<String> {
        match def.object_id(id) {
            Ok(ObjectId::Singleton) => Ok(ObjectId::Singleton.as_str().to_string()),
            Ok(ObjectId::Id(id)) => Ok(id.to_string()),
            Err(IdError::BadSingleton) => Err(Error::BadSingletonId),
            Err(IdError::Missing) => Err(PlanError::MissingUpdateId {
                op: self,
                object: def.display_name().to_string(),
            }
            .into()),
        }
    }
}
