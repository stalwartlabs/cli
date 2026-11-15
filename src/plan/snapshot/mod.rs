/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod graph;
pub mod transform;

use crate::error::Result;
use crate::jmap::Client;
use crate::jmap::response::GetResponse;
use crate::plan::AT_TYPE;
use crate::plan::error::SnapshotError;
use crate::plan::event::SnapshotEvent;
use crate::plan::format::{PlanRef, PlanWriter};
use crate::plan::store::{ObjectStore, object_id};
use crate::schema::Schema;
use crate::schema::object::ObjectId;
use crate::schema::object::display_name;
use graph::{ShardFields, ShardGraph, ShardKey};
use serde_json::Map;
use std::collections::HashSet;
use std::io::Write;
use std::ops::ControlFlow;
use transform::Transformer;

pub use transform::Secrets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotStats {
    pub upserts: usize,
    pub singletons: usize,
}

pub struct Snapshot<'s> {
    schema: &'s Schema,
    graph: ShardGraph<'s>,
    allow: HashSet<&'s str>,
}

struct Writer<'w, 'o, 's> {
    client: &'w Client,
    plan: PlanWriter<'o>,
    on: &'w mut dyn FnMut(SnapshotEvent<'_>) -> ControlFlow<()>,
    warned_value_match: HashSet<&'s str>,
    warned_secrets: HashSet<&'s str>,
}

impl<'s> Writer<'_, '_, 's> {
    fn emit(&mut self, event: SnapshotEvent<'_>) -> Result<()> {
        self.plan.flush()?;
        match (self.on)(event) {
            ControlFlow::Continue(()) => Ok(()),
            ControlFlow::Break(()) => Err(SnapshotError::Cancelled.into()),
        }
    }

    fn warn_anonymized(
        &mut self,
        object: &'s str,
        transformer: &mut Transformer<'s, '_>,
    ) -> Result<()> {
        if transformer.take_anonymized() && self.warned_secrets.insert(object) {
            self.emit(SnapshotEvent::AnonymizedSecrets { object })?;
        }
        Ok(())
    }
}

impl<'s> Snapshot<'s> {
    pub fn build(
        schema: &'s Schema,
        objects: &[String],
        allow_unresolved: &[String],
    ) -> Result<Snapshot<'s>> {
        let mut selection: Vec<&'s str> = Vec::with_capacity(objects.len());
        for raw in objects {
            if raw.contains('/') {
                return Err(SnapshotError::VariantSelected(raw.clone()).into());
            }
            let canonical = schema.object(raw)?.name();
            if !selection.contains(&canonical) {
                selection.push(canonical);
            }
        }
        let mut allow: HashSet<&'s str> = HashSet::with_capacity(allow_unresolved.len());
        for raw in allow_unresolved {
            let raw = raw.trim();
            if !raw.is_empty() {
                allow.insert(schema.object(raw)?.name());
            }
        }
        ShardGraph::validate_static_refs(schema, &selection, &allow)?;
        let graph = ShardGraph::build(schema, &selection)?;
        Ok(Snapshot {
            schema,
            graph,
            allow,
        })
    }

    pub fn stats(&self) -> SnapshotStats {
        SnapshotStats {
            upserts: self.graph.shards.len(),
            singletons: self.graph.singletons.len(),
        }
    }

    pub fn write(
        &self,
        client: &Client,
        secrets: Secrets,
        out: &mut dyn Write,
        on: &mut dyn FnMut(SnapshotEvent<'_>) -> ControlFlow<()>,
    ) -> Result<()> {
        self.write_with(client, ObjectStore::default(), secrets, out, on)
    }

    fn write_with(
        &self,
        client: &Client,
        mut cache: ObjectStore<'s>,
        secrets: Secrets,
        out: &mut dyn Write,
        on: &mut dyn FnMut(SnapshotEvent<'_>) -> ControlFlow<()>,
    ) -> Result<()> {
        let mut writer = Writer {
            client,
            plan: PlanWriter::new(out),
            on,
            warned_value_match: HashSet::new(),
            warned_secrets: HashSet::new(),
        };
        let mut transformer = Transformer::new(self.schema, &self.allow, secrets);
        for &shard in &self.graph.shards {
            self.upsert(&mut writer, &mut transformer, &mut cache, shard)?;
        }
        self.deferred_updates(&mut writer, &mut transformer, &cache)?;
        for &object in &self.graph.singletons {
            self.singleton(&mut writer, &mut transformer, object)?;
        }
        writer.plan.flush()
    }

    fn upsert(
        &self,
        writer: &mut Writer<'_, '_, 's>,
        transformer: &mut Transformer<'s, '_>,
        cache: &mut ObjectStore<'s>,
        shard: ShardKey<'s>,
    ) -> Result<()> {
        let fields = match ShardFields::of(self.schema, shard) {
            ShardFields::Missing => {
                return Err(SnapshotError::MissingShardSchema(shard.object.to_string()).into());
            }
            fields => fields.fields(),
        };
        writer.plan.flush()?;
        if !cache.contains(shard.object) {
            writer.emit(SnapshotEvent::Fetching {
                object: shard.object,
            })?;
            let count = cache.fetch(writer.client, shard.object)?;
            writer.emit(SnapshotEvent::Fetched { count })?;
        }

        let mut objects = cache.partition(shard.object, shard.variant).peekable();
        let done = |count| SnapshotEvent::ShardDone {
            object: shard.object,
            variant: shard.variant,
            count,
        };
        if objects.peek().is_none() {
            return writer.emit(done(0));
        }

        let match_on = self
            .schema
            .get_object(shard.object)
            .and_then(|def| def.label_property());
        if match_on.is_none() && writer.warned_value_match.insert(shard.object) {
            writer.emit(SnapshotEvent::ValueMatchFallback {
                object: shard.object,
            })?;
        }

        let omit: Vec<&str> = self.graph.deferred_from(shard).map(|d| d.field).collect();
        writer
            .plan
            .begin_upsert(display_name(shard.object), match_on)?;
        let mut total = 0usize;
        for obj in objects {
            let Some(id) = object_id(obj) else {
                continue;
            };
            let mut body = transformer.object(fields, obj);
            if shard.variant.is_some()
                && let Some(at_type) = obj.get(AT_TYPE)
            {
                body.insert(AT_TYPE.into(), at_type.clone());
            }
            for field in &omit {
                body.remove(*field);
            }
            if let Some(key) = match_on
                && !body.contains_key(key)
                && let Some(v) = obj.get(key)
            {
                body.insert(key.to_string(), v.clone());
            }
            writer.plan.entry(
                PlanRef {
                    prefix: transformer.prefix(shard.object),
                    id,
                },
                &body,
            )?;
            total += 1;
        }
        writer.plan.end_upsert()?;
        writer.plan.flush()?;
        writer.warn_anonymized(shard.object, transformer)?;
        writer.emit(done(total))
    }

    fn deferred_updates(
        &self,
        writer: &mut Writer<'_, '_, 's>,
        transformer: &mut Transformer<'s, '_>,
        cache: &ObjectStore<'s>,
    ) -> Result<()> {
        let mut groups: Vec<(ShardKey<'s>, Vec<&'s str>)> = Vec::new();
        for edge in &self.graph.deferred {
            match groups.iter_mut().find(|(key, _)| *key == edge.from) {
                Some((_, fields)) => {
                    if !fields.contains(&edge.field) {
                        fields.push(edge.field);
                    }
                }
                None => groups.push((edge.from, vec![edge.field])),
            }
        }
        for (key, names) in groups {
            let fields = match ShardFields::of(self.schema, key) {
                ShardFields::Missing => continue,
                fields => fields.fields(),
            };
            let Some(fields) = fields else {
                continue;
            };
            let deferred: Vec<_> = names
                .iter()
                .filter_map(|name| fields.properties.get_key_value(*name))
                .collect();
            let mut patch = Map::with_capacity(deferred.len());
            for obj in cache.partition(key.object, key.variant) {
                let Some(id) = object_id(obj) else {
                    continue;
                };
                patch.clear();
                for &(name, field) in &deferred {
                    transformer.field_into(name, field, obj, &mut patch);
                }
                if patch.is_empty() {
                    continue;
                }
                writer.plan.update(
                    display_name(key.object),
                    Some(PlanRef {
                        prefix: transformer.prefix(key.object),
                        id,
                    }),
                    &patch,
                )?;
            }
        }
        transformer.take_anonymized();
        Ok(())
    }

    fn singleton(
        &self,
        writer: &mut Writer<'_, '_, 's>,
        transformer: &mut Transformer<'s, '_>,
        object: &'s str,
    ) -> Result<()> {
        let def = self
            .schema
            .get_object(object)
            .filter(|_| self.schema.schemas.contains_key(object))
            .ok_or_else(|| SnapshotError::MissingSingletonSchema(object.to_string()))?;
        writer.emit(SnapshotEvent::FetchingSingleton { object })?;
        let ids = [ObjectId::Singleton.as_str()];
        let GetResponse { list } = writer.client.get(object, &ids, None)?;
        let Some(obj) = list.into_iter().next() else {
            return Err(SnapshotError::EmptySingleton(def.display_name().to_string()).into());
        };
        let fields = def
            .typed_fields(&obj)
            .map_err(|e| SnapshotError::singleton_variant(def, e))?;
        let body = transformer.object(fields, &obj);
        writer.plan.update(def.display_name(), None, &body)?;
        writer.warn_anonymized(object, transformer)
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/plan/snapshot/tests/mod.rs"]
mod tests;
