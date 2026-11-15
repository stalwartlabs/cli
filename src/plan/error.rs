/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Error;
use crate::jmap::set_error::SetError;
use crate::plan::apply::Summary;
use crate::plan::op::OpKind;
use crate::schema::ObjectDef;
use crate::schema::object::{VariantError, display_name};
use crate::view::{RefLabels, Styled, set_error_body};
use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpRef {
    pub index: usize,
    pub kind: OpKind,
}

impl OpRef {
    pub fn number(self) -> usize {
        self.index + 1
    }
}

impl fmt::Display for OpRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} operation #{}", self.kind.as_str(), self.number())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefSite {
    Match,
    Scope,
}

impl fmt::Display for RefSite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RefSite::Match => "match property",
            RefSite::Scope => "`scope` property",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("invalid plan NDJSON on line {line}: {source}")]
    Syntax {
        line: usize,
        source: serde_json::Error,
    },
    #[error(
        "cannot {} singleton `{object}` (operation #{}){}",
        op.kind.as_str(),
        op.number(),
        if op.kind == OpKind::Destroy { "" } else { "; use update instead" }
    )]
    Singleton { op: OpRef, object: String },
    #[error("{op} has an empty `value` map (no objects to {})", op.kind.as_str())]
    EmptyValue { op: OpRef },
    #[error("{op} has duplicate id `{id}`")]
    DuplicateId { op: OpRef, id: String },
    #[error(
        "{op} on `{object}` has no match key; add a `matchOn` (or `\"matchOn\": \"*\"` to match \
         by value, which deletes and recreates any drifted object)"
    )]
    NoMatchKey { op: OpRef, object: String },
    #[error(
        "{op} on `{object}` is missing the top-level `id` field (required for non-singletons; \
         `id` is a sibling of `value`, not a key inside it)"
    )]
    MissingUpdateId { op: OpRef, object: String },
    #[error("{op} has a `scope`; only upsert and reconcile match against existing objects")]
    ScopeNotAllowed { op: OpRef },
    #[error("{op} has a `scope` that is not a JSON object")]
    ScopeNotObject { op: OpRef },
    #[error(
        "{op} has a null `scope` value for `{property}`; a null matches every object that does \
         not set the property, which would widen the scope rather than narrow it"
    )]
    ScopeNull { op: OpRef, property: String },
    #[error("{op} has a `scope` on `@type`, but {object} is not a multi-variant type")]
    ScopeNotMultiVariant { op: OpRef, object: String },
    #[error(
        "{op} has a `scope` on `@type` = {wanted}, which is not a variant of {object}; expected \
         one of {expected}"
    )]
    ScopeUnknownVariant {
        op: OpRef,
        wanted: Box<Value>,
        object: String,
        expected: String,
    },
    #[error(
        "{op} has a `scope` on `{property}`, which the server derives; an entry cannot be \
         created into that scope, so it would be recreated on every apply"
    )]
    ScopeServerSet { op: OpRef, property: String },
    #[error(
        "{op} has a `scope` on `{property}`, which is not a property of {object}; the scope is \
         matched client-side against property values, so operator forms such as \
         `{property}Contains` are not supported"
    )]
    ScopeUnknownProperty {
        op: OpRef,
        property: String,
        object: String,
    },
    #[error(
        "{op} declares `{client_id}` with `{property}` = {actual}, which is outside its own \
         `scope` ({property} = {wanted}); an out-of-scope entry can never match an existing \
         object, so it would be created again on every apply. Move it to its own operation."
    )]
    EntryOutsideScope {
        op: OpRef,
        client_id: String,
        property: String,
        actual: Box<Value>,
        wanted: Box<Value>,
    },
    #[error("{op} has an empty `matchOn` list")]
    EmptyMatchOn { op: OpRef },
    #[error(
        "{op} has an invalid `matchOn` value `{value}`; expected a list of property names or \
         `\"*\"`"
    )]
    InvalidMatchOn { op: OpRef, value: String },
    #[error(
        "{object}: {site} `{property}` references unresolved id `#{client_id}` (no create, \
         upsert, or reconcile operation in this plan produced it)"
    )]
    UnresolvedRef {
        object: String,
        site: RefSite,
        property: String,
        client_id: String,
    },
}

impl PlanError {
    pub fn unresolved(def: ObjectDef<'_>, site: RefSite, property: &str, client_id: &str) -> Self {
        PlanError::UnresolvedRef {
            object: def.display_name().to_string(),
            site,
            property: property.to_string(),
            client_id: client_id.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetAction {
    Create(OpRef),
    Update,
    Destroy,
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("{object}: match property `{property}` is missing from the object body")]
    MissingMatchProperty { object: String, property: String },
    #[error("{object}: upsert entry `{client_id}` is not a JSON object")]
    EntryNotObject { object: String, client_id: String },
    #[error("{object}: upsert entry is missing `@type` (required to match a multi-variant object)")]
    MissingVariant { object: String },
    #[error("{object}: ambiguous upsert; {count} existing objects match on {properties}")]
    AmbiguousMatch {
        object: String,
        count: usize,
        properties: String,
    },
    #[error(
        "{object}: ambiguous match; {count} existing objects have the same values; add a \
         `matchOn` naming the properties that identify the object"
    )]
    AmbiguousValueMatch { object: String, count: usize },
    #[error(
        "{object}: cannot determine a match key (no label property and no schema fields); add a \
         `matchOn` to the plan"
    )]
    NoMatchFields { object: String },
    #[error("{object}: no comparable properties to match on; add a `matchOn` to the plan")]
    NoComparableProperties { object: String },
    #[error(
        "{object}: operation #{} has two entries (`{first}`, `{second}`) with identical \
         values; value matching cannot tell them apart, so the second would be created again on \
         every apply",
        .op.number()
    )]
    DuplicateValues {
        object: String,
        op: OpRef,
        first: String,
        second: String,
    },
    #[error(
        "{object}: operation #{} has two entries (`{first}`, `{second}`) with the same \
         match key on {properties}; `matchOn` must uniquely identify each entry",
        .op.number()
    )]
    DuplicateMatchKey {
        object: String,
        op: OpRef,
        first: String,
        second: String,
        properties: String,
    },
    #[error(
        "{object}: update references unknown created id `#{client_id}` (no matching create \
         operation in this plan)"
    )]
    UnknownUpdateRef { object: String, client_id: String },
    #[error("{object}: server returned no `updated` entry for id `{id}` (the id may not exist)")]
    NotUpdated { object: String, id: String },
    #[error("{0}")]
    SetFailed(Box<SetFailure>),
    #[error("apply completed with {failed} failed operation(s)")]
    Incomplete { failed: usize },
    #[error(
        "apply cancelled after {} created, {} updated, {} destroyed",
        .0.created,
        .0.updated,
        .0.destroyed
    )]
    Cancelled(Summary),
}

#[derive(Debug)]
pub struct SetFailure {
    pub object: String,
    pub id: String,
    pub action: SetAction,
    pub error: SetError,
    pub labels: RefLabels,
}

impl SetFailure {
    pub fn new(
        object: &str,
        id: &str,
        action: SetAction,
        error: &SetError,
        labels: RefLabels,
    ) -> Self {
        SetFailure {
            object: display_name(object).to_string(),
            id: id.to_string(),
            action,
            error: error.clone(),
            labels,
        }
    }
}

impl From<SetFailure> for ApplyError {
    fn from(failure: SetFailure) -> Self {
        ApplyError::SetFailed(Box::new(failure))
    }
}

impl From<SetFailure> for Error {
    fn from(failure: SetFailure) -> Self {
        ApplyError::from(failure).into()
    }
}

impl fmt::Display for SetFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let SetFailure {
            object,
            id,
            action,
            error,
            labels,
        } = self;
        match action {
            SetAction::Create(op) => write!(
                f,
                "{object}: create failed for `{id}` (operation #{}): ",
                op.number()
            )?,
            SetAction::Update => write!(f, "{object}: update failed for id {id}: ")?,
            SetAction::Destroy => write!(f, "{object}: destroy failed for id {id}: ")?,
        }
        let mut body = Styled::default();
        set_error_body(error, labels, &mut body);
        let rendered = body.into_text();
        let mut lines = rendered.split('\n');
        if let Some(first) = lines.next() {
            f.write_str(first)?;
        }
        for line in lines {
            f.write_str("\n  ")?;
            f.write_str(line)?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("snapshot cannot select variants individually ({0}); use the bare object name")]
    VariantSelected(String),
    #[error("{0} is a view and cannot be snapshotted directly")]
    View(String),
    #[error("{from} references {to} but {to} is not in the snapshot selection; {remedy}")]
    UnselectedReference {
        from: String,
        to: String,
        remedy: Remedy,
    },
    #[error("cannot snapshot: cyclic dependency between selected types: {nodes}{hint}")]
    Cycle { nodes: String, hint: CycleHint },
    #[error("cannot snapshot: dependency graph still has cycles after cycle-breaking")]
    UnbrokenCycle,
    #[error("snapshot cancelled")]
    Cancelled,
    #[error("singleton {0} returned no value")]
    EmptySingleton(String),
    #[error("unexpected server response: schema missing entry for {0}")]
    MissingSchemaEntry(String),
    #[error("unexpected server response: no schema available for {0}")]
    MissingShardSchema(String),
    #[error("unexpected server response: singleton schema missing for {0}")]
    MissingSingletonSchema(String),
    #[error("unexpected server response: multi-variant singleton {0} returned no @type")]
    UntypedSingleton(String),
    #[error("unexpected server response: unknown variant {variant} for singleton {object}")]
    UnknownSingletonVariant { object: String, variant: String },
}

impl SnapshotError {
    pub fn singleton_variant(def: ObjectDef<'_>, error: VariantError<'_>) -> Self {
        let object = def.display_name().to_string();
        match error {
            VariantError::Missing => SnapshotError::UntypedSingleton(object),
            VariantError::Unknown(variant) => SnapshotError::UnknownSingletonVariant {
                object,
                variant: variant.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remedy {
    Select(String),
    AllowUnresolved(String),
}

impl fmt::Display for Remedy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Remedy::Select(to) => write!(
                f,
                "add {to} to the selection, or use --allow-unresolved {to}"
            ),
            Remedy::AllowUnresolved(to) => write!(
                f,
                "adding {to} to the selection would form a cycle, so use --allow-unresolved {to}"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CycleHint {
    None,
    Immutable {
        from: String,
        field: String,
        to: String,
    },
}

impl fmt::Display for CycleHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CycleHint::None => Ok(()),
            CycleHint::Immutable { from, field, to } => write!(
                f,
                " (immutable field closes the cycle: {from}.{field} -> {to})"
            ),
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/plan/error/tests.rs"]
mod tests;
