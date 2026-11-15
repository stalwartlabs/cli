/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::jmap::Client;
use crate::plan::AT_TYPE;
use crate::plan::error::{ApplyError, OpRef};
use crate::plan::event::{ApplyEvent, BatchAction};
use crate::plan::exec::{Batch, Created, Exec, batches};
use crate::plan::matcher::{MatchKey, OpMatcher};
use crate::plan::op::{Action, Merge, Op, OpKind, Plan, PlanStats};
use crate::plan::refs::ResolvedScope;
use crate::plan::store::{Object, ObjectStore};
use crate::schema::Field;
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnError {
    Stop,
    Continue,
}

#[derive(Debug, Clone, Copy)]
pub struct ApplyOptions {
    pub on_error: OnError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub planned: PlanStats,
    pub destroyed: usize,
    pub updated: usize,
    pub created: usize,
    pub failed: usize,
}

#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    created: usize,
    updated: usize,
    destroyed: usize,
}

impl Counts {
    fn total(self) -> usize {
        self.created + self.updated + self.destroyed
    }
}

#[derive(Default)]
struct TypedIds<'s>(HashMap<&'s str, HashSet<String>>);

impl<'s> TypedIds<'s> {
    fn insert(&mut self, object: &'s str, id: &str) -> bool {
        let ids = self.0.entry(object).or_default();
        !ids.contains(id) && ids.insert(id.to_string())
    }

    fn contains(&self, object: &str, id: &str) -> bool {
        self.0.get(object).is_some_and(|ids| ids.contains(id))
    }
}

struct MergeOutcome<'p> {
    counts: Counts,
    matched: HashSet<String>,
    scope: Option<ResolvedScope<'p>>,
}

struct Leaks<'s> {
    index: usize,
    object: &'s str,
    ids: Vec<String>,
}

struct Applier<'a, 's> {
    exec: Exec<'a>,
    on_error: OnError,
    on: &'a mut dyn FnMut(ApplyEvent<'_>) -> ControlFlow<()>,
    flow: ControlFlow<()>,
    touched: TypedIds<'s>,
    cache: ObjectStore<'s>,
    warned: HashSet<&'s str>,
    summary: Summary,
}

impl<'s> Plan<'s> {
    pub fn apply(
        &self,
        client: &Client,
        opts: ApplyOptions,
        on: &mut dyn FnMut(ApplyEvent<'_>) -> ControlFlow<()>,
    ) -> Result<Summary> {
        let mut applier = Applier {
            exec: Exec::new(client),
            on_error: opts.on_error,
            on,
            flow: ControlFlow::Continue(()),
            touched: TypedIds::default(),
            cache: ObjectStore::default(),
            warned: HashSet::new(),
            summary: Summary {
                planned: *self.stats(),
                destroyed: 0,
                updated: 0,
                created: 0,
                failed: 0,
            },
        };
        applier.run(&self.ops)?;
        Ok(applier.summary)
    }
}

impl<'a, 's> Applier<'a, 's> {
    fn run(&mut self, ops: &[Op<'s>]) -> Result<()> {
        for op in ops.iter().rev() {
            if let Action::Destroy { filter } = &op.action {
                self.checkpoint()?;
                let result = self.destroy(op.object(), filter.as_ref());
                self.record(op.index, OpKind::Destroy, op.object(), result)?;
            }
        }

        let mut leaks: Vec<Leaks<'s>> = Vec::new();
        for op in ops {
            self.checkpoint()?;
            let result = match &op.action {
                Action::Destroy { .. } => continue,
                Action::Create(value) => self.create(op, value),
                Action::Update { id, value } => self.update(op.object(), id, value),
                Action::Upsert(merge) => self.merge(op, merge).map(|outcome| outcome.counts),
                Action::Reconcile(merge) => self.merge(op, merge).map(|outcome| {
                    leaks.push(Leaks {
                        index: op.index,
                        object: op.object(),
                        ids: self.cache.leaked(
                            op.object(),
                            &outcome.matched,
                            outcome.scope.as_ref(),
                        ),
                    });
                    outcome.counts
                }),
            };
            self.record(op.index, op.kind(), op.object(), result)?;
        }

        let mut destroyed = TypedIds::default();
        for mut leak in leaks.into_iter().rev() {
            leak.ids.retain(|id| {
                !self.touched.contains(leak.object, id) && destroyed.insert(leak.object, id)
            });
            if leak.ids.is_empty() {
                continue;
            }
            self.checkpoint()?;
            let result = self.destroy_ids(leak.object, &leak.ids);
            match result {
                Ok(count) => {
                    emit(
                        self.on,
                        &mut self.flow,
                        ApplyEvent::ReconcileCleanup {
                            index: leak.index,
                            object: leak.object,
                            destroyed: count,
                        },
                    );
                }
                Err(e) => self.fail(leak.index, OpKind::Reconcile, leak.object, e)?,
            }
        }

        emit(self.on, &mut self.flow, ApplyEvent::Finished(&self.summary));
        Ok(())
    }

    fn checkpoint(&self) -> Result<()> {
        match self.flow {
            ControlFlow::Continue(()) => Ok(()),
            ControlFlow::Break(()) => Err(ApplyError::Cancelled(self.summary).into()),
        }
    }

    fn record(
        &mut self,
        index: usize,
        kind: OpKind,
        object: &str,
        result: Result<Counts>,
    ) -> Result<()> {
        match result {
            Ok(counts) => {
                emit(
                    self.on,
                    &mut self.flow,
                    ApplyEvent::OpDone {
                        index,
                        kind,
                        object,
                        count: counts.total(),
                    },
                );
                Ok(())
            }
            Err(e) => self.fail(index, kind, object, e),
        }
    }

    fn fail(&mut self, index: usize, kind: OpKind, object: &str, error: Error) -> Result<()> {
        if is_cancelled(&error) {
            return Err(error);
        }
        self.summary.failed += 1;
        emit(
            self.on,
            &mut self.flow,
            ApplyEvent::OpFailed {
                index,
                kind,
                object,
                error: &error,
            },
        );
        match self.on_error {
            OnError::Continue => Ok(()),
            OnError::Stop => {
                emit(self.on, &mut self.flow, ApplyEvent::Finished(&self.summary));
                Err(error)
            }
        }
    }

    fn batch_reporter(
        on: &mut dyn FnMut(ApplyEvent<'_>) -> ControlFlow<()>,
        flow: &mut ControlFlow<()>,
        action: BatchAction,
        object: &str,
    ) -> impl FnMut(Batch) -> ControlFlow<()> {
        move |b: Batch| {
            emit(
                on,
                flow,
                ApplyEvent::Batch {
                    action,
                    object,
                    number: b.number,
                    of: b.of,
                    size: b.size,
                },
            );
            *flow
        }
    }

    fn destroy(&mut self, object: &'s str, filter: Option<&Value>) -> Result<Counts> {
        let ids = self.exec.fetch_ids(object, filter)?;
        let destroyed = self.destroy_ids(object, &ids)?;
        Ok(Counts {
            destroyed,
            ..Counts::default()
        })
    }

    fn destroy_ids(&mut self, object: &'s str, ids: &[String]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let (destroyed, result) = {
            let mut on_batch =
                Self::batch_reporter(self.on, &mut self.flow, BatchAction::Destroy, object);
            self.exec.destroy(object, ids, &mut on_batch)
        };
        self.cache.invalidate(object);
        self.summary.destroyed += destroyed;
        result?;
        self.checkpoint()?;
        Ok(destroyed)
    }

    fn create(&mut self, op: &Op<'s>, value: &Map<String, Value>) -> Result<Counts> {
        let (objects, result) = self.create_objects(op.object(), value, op.op_ref());
        let created = self.confirm_created(op.object(), objects);
        result?;
        self.checkpoint()?;
        Ok(Counts {
            created,
            ..Counts::default()
        })
    }

    fn confirm_created(&mut self, object: &'s str, objects: Vec<Created<'_>>) -> usize {
        for id in objects.iter().filter_map(Created::id) {
            self.touched.insert(object, id);
        }
        let created = objects.len();
        self.summary.created += created;
        self.cache
            .record_created(object, objects.into_iter().map(Created::into_object));
        created
    }

    fn create_objects<'v>(
        &mut self,
        object: &'s str,
        value: &'v Map<String, Value>,
        op: OpRef,
    ) -> (Vec<Created<'v>>, Result<()>) {
        let mut on_batch =
            Self::batch_reporter(self.on, &mut self.flow, BatchAction::Create, object);
        self.exec.create(object, value, op, &mut on_batch)
    }

    fn update(&mut self, object: &'s str, id: &str, value: &Value) -> Result<Counts> {
        let (updated, real_id) = self.exec.update(object, id, value)?;
        self.summary.updated += updated;
        if let Some(patch) = value.as_object() {
            self.cache.record_updated(object, &real_id, patch);
        }
        self.touched.insert(object, &real_id);
        Ok(Counts {
            updated,
            ..Counts::default()
        })
    }

    fn merge<'p>(&mut self, op: &'p Op<'s>, merge: &'p Merge) -> Result<MergeOutcome<'p>> {
        let object = op.object();
        let def = op.def;
        self.cache.ensure(self.exec.client(), object)?;
        let scope = merge
            .scope
            .as_ref()
            .map(|s| self.exec.ids.resolve_scope(def, s))
            .transpose()?;
        let key = MatchKey::resolve(def, merge.match_on.as_ref());
        if matches!(key, MatchKey::Value) && merge.match_on.is_none() && self.warned.insert(object)
        {
            emit(
                self.on,
                &mut self.flow,
                ApplyEvent::ValueMatchFallback { object },
            );
        }

        let mut entries: Vec<(&String, Cow<'_, Object>)> = Vec::with_capacity(merge.value.len());
        for (client_id, body) in &merge.value {
            let body = body.as_object().ok_or_else(|| ApplyError::EntryNotObject {
                object: def.display_name().to_string(),
                client_id: client_id.clone(),
            })?;
            let body = match &scope {
                Some(scope) => scope.fill(body),
                None => Cow::Borrowed(body),
            };
            entries.push((client_id, body));
        }

        let mut to_create: Map<String, Value> = Map::new();
        let mut to_update: Vec<(String, Value)> = Vec::new();
        let mut matched: HashSet<String> = HashSet::new();
        {
            let mut matcher =
                OpMatcher::new(def, key, scope.as_ref(), self.cache.objects_for(object));
            matcher.reject_duplicates(&entries, &self.exec.ids, op.op_ref())?;
            for (client_id, body) in &entries {
                let body = body.as_ref();
                let fields = def.fields_of(body);
                match matcher.find(body, &self.exec.ids)? {
                    Some(server_id) => {
                        self.touched.insert(object, server_id);
                        matched.insert(server_id.to_string());
                        self.exec
                            .ids
                            .insert((*client_id).clone(), server_id.to_string());
                        let patch: Object = body
                            .iter()
                            .filter(|(k, _)| {
                                k.as_str() != AT_TYPE
                                    && fields.is_none_or(|fields| {
                                        fields.properties.get(*k).is_none_or(Field::is_patchable)
                                    })
                            })
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        if !patch.is_empty() {
                            to_update.push((server_id.to_string(), Value::Object(patch)));
                        }
                    }
                    None => {
                        let create_body: Object = body
                            .iter()
                            .filter(|(k, _)| {
                                fields.is_none_or(|fields| {
                                    fields.properties.get(*k).is_none_or(|f| !f.is_server_set())
                                })
                            })
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        to_create.insert((*client_id).clone(), Value::Object(create_body));
                    }
                }
            }
        }

        let mut counts = Counts::default();
        if !to_create.is_empty() {
            let (objects, result) = self.create_objects(object, &to_create, op.op_ref());
            matched.extend(objects.iter().filter_map(Created::id).map(str::to_string));
            counts.created = self.confirm_created(object, objects);
            result?;
            self.checkpoint()?;
        }
        let updates: Vec<(&str, &Value)> = to_update
            .iter()
            .map(|(server_id, body)| (server_id.as_str(), body))
            .collect();
        for (batch, chunk) in batches(&updates, self.exec.client().max_set()) {
            emit(
                self.on,
                &mut self.flow,
                ApplyEvent::Batch {
                    action: BatchAction::Update,
                    object,
                    number: batch.number,
                    of: batch.of,
                    size: batch.size,
                },
            );
            self.checkpoint()?;
            let (updated, result) = self.exec.update_batch(object, chunk);
            counts.updated += updated.len();
            self.summary.updated += updated.len();
            for (server_id, body) in chunk {
                if let Some(patch) = body.as_object()
                    && updated.contains_key(*server_id)
                {
                    self.cache.record_updated(object, server_id, patch);
                }
            }
            result?;
        }
        Ok(MergeOutcome {
            counts,
            matched,
            scope,
        })
    }
}

fn emit(
    on: &mut dyn FnMut(ApplyEvent<'_>) -> ControlFlow<()>,
    flow: &mut ControlFlow<()>,
    event: ApplyEvent<'_>,
) {
    if on(event).is_break() {
        *flow = ControlFlow::Break(());
    }
}

fn is_cancelled(error: &Error) -> bool {
    matches!(error, Error::Apply(e) if matches!(**e, ApplyError::Cancelled(_)))
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/plan/apply/tests.rs"]
mod tests;
