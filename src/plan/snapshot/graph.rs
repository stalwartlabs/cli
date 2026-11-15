/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::plan::error::{CycleHint, Remedy, SnapshotError};
use crate::schema::object::{display_name, split_view};
use crate::schema::{
    Field, FieldType, FieldUpdate, Fields, MapValueType, ObjectSchema, ObjectType, ScalarType,
    Schema,
};
use std::collections::btree_map::Keys;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ShardKey<'s> {
    pub object: &'s str,
    pub variant: Option<&'s str>,
}

impl fmt::Display for ShardKey<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(display_name(self.object))?;
        if let Some(v) = self.variant {
            write!(f, "/{v}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ShardFields<'s> {
    Missing,
    Empty,
    Fields(&'s Fields),
}

impl<'s> ShardFields<'s> {
    pub fn of(schema: &'s Schema, key: ShardKey<'_>) -> Self {
        let Some(def) = schema.get_object(key.object) else {
            return ShardFields::Missing;
        };
        let payloadless = def.is_multi_variant()
            && key
                .variant
                .and_then(|name| def.variant(name))
                .is_some_and(|v| v.schema_name.is_none());
        if payloadless {
            return ShardFields::Empty;
        }
        def.fields_for(key.variant)
            .map_or(ShardFields::Missing, ShardFields::Fields)
    }

    pub fn fields(self) -> Option<&'s Fields> {
        match self {
            ShardFields::Fields(f) => Some(f),
            ShardFields::Missing | ShardFields::Empty => None,
        }
    }

    fn properties(self) -> impl Iterator<Item = (&'s String, &'s Field)> {
        self.fields().into_iter().flat_map(|f| f.properties.iter())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arity {
    One,
    Many,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mutability {
    Mutable,
    Fixed,
}

struct Walker<'s, 'o> {
    schema: &'s Schema,
    visited: &'o mut HashSet<&'s str>,
    out: &'o mut Vec<(ShardKey<'s>, Arity)>,
}

impl<'s> Walker<'s, '_> {
    fn field_type(&mut self, t: &'s FieldType, arity: Arity) {
        match t {
            FieldType::ObjectId { object_name, .. } => self.targets(object_name, arity),
            FieldType::Set {
                class: ScalarType::ObjectId { object_name },
                ..
            } => self.targets(object_name, Arity::Many),
            FieldType::Map {
                key_class,
                value_class,
                ..
            } => {
                if let ScalarType::ObjectId { object_name } = key_class {
                    self.targets(object_name, Arity::Many);
                }
                if let MapValueType::Object { object_name } = value_class {
                    self.embedded(object_name, Arity::Many);
                }
            }
            FieldType::Object { object_name, .. } => self.embedded(object_name, arity),
            FieldType::ObjectList { object_name, .. } => self.embedded(object_name, Arity::Many),
            _ => {}
        }
    }

    fn fields(&mut self, fields: &'s Fields, arity: Arity) {
        for field in fields.properties.values() {
            self.field_type(&field.typ, arity);
        }
    }

    fn targets(&mut self, object_name: &'s str, arity: Arity) {
        let (canonical, variant) = split_view(object_name);
        if let Some(ObjectType::View {
            object_name: parent,
        }) = self.schema.objects.get(canonical)
        {
            let parent_variant = object_name.rsplit_once('/').map(|(_, v)| v);
            self.out.push((
                ShardKey {
                    object: parent,
                    variant: parent_variant,
                },
                arity,
            ));
            return;
        }
        match self.schema.schemas.get(canonical) {
            Some(ObjectSchema::Multiple { variants }) => match variant {
                Some(v) => self.out.push((
                    ShardKey {
                        object: canonical,
                        variant: Some(v),
                    },
                    arity,
                )),
                None => self.out.extend(variants.iter().map(|v| {
                    (
                        ShardKey {
                            object: canonical,
                            variant: Some(v.name.as_str()),
                        },
                        arity,
                    )
                })),
            },
            _ => self.out.push((
                ShardKey {
                    object: canonical,
                    variant: None,
                },
                arity,
            )),
        }
    }

    fn embedded(&mut self, object_name: &'s str, arity: Arity) {
        if self.visited.insert(object_name) {
            self.object(object_name, arity);
        }
    }

    fn object(&mut self, object_name: &str, arity: Arity) {
        let schema = self.schema;
        let (single, variants) = match schema.schemas.get(object_name) {
            Some(ObjectSchema::Single { schema_name }) => (schema.fields.get(schema_name), &[][..]),
            Some(ObjectSchema::Multiple { variants }) => (None, variants.as_slice()),
            None => (None, &[][..]),
        };
        let variants = variants
            .iter()
            .filter_map(|v| v.schema_name.as_ref().and_then(|sn| schema.fields.get(sn)));
        for fields in single.into_iter().chain(variants) {
            self.fields(fields, arity);
        }
    }
}

#[derive(Debug, Clone)]
struct EdgeField<'s> {
    name: &'s str,
    mutability: Mutability,
    arity: Arity,
}

#[derive(Default, Debug, Clone)]
struct EdgeFields<'s> {
    fields: Vec<EdgeField<'s>>,
}

impl<'s> EdgeFields<'s> {
    fn add(&mut self, field: EdgeField<'s>) {
        if !self.fields.iter().any(|f| f.name == field.name) {
            self.fields.push(field);
        }
    }

    fn pick_mutable(&self) -> Option<&EdgeField<'s>> {
        let mut chosen: Option<&EdgeField<'s>> = None;
        for f in self
            .fields
            .iter()
            .filter(|f| f.mutability == Mutability::Mutable)
        {
            match chosen {
                None => chosen = Some(f),
                Some(c) if c.arity == Arity::One && f.arity == Arity::Many => chosen = Some(f),
                _ => {}
            }
        }
        chosen
    }

    fn any_fixed(&self) -> Option<&'s str> {
        self.fields
            .iter()
            .find(|f| f.mutability == Mutability::Fixed)
            .map(|f| f.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deferred<'s> {
    pub from: ShardKey<'s>,
    pub to: ShardKey<'s>,
    pub field: &'s str,
}

pub struct ShardGraph<'s> {
    pub shards: Vec<ShardKey<'s>>,
    pub singletons: Vec<&'s str>,
    pub deferred: Vec<Deferred<'s>>,
}

impl<'s> ShardGraph<'s> {
    pub fn build(schema: &'s Schema, selection: &[&'s str]) -> Result<Self> {
        let mut shards: Vec<ShardKey<'s>> = Vec::new();
        let mut singletons: Vec<&'s str> = Vec::new();
        for &object in selection {
            match schema.objects.get(object) {
                None => {
                    return Err(SnapshotError::MissingSchemaEntry(object.to_string()).into());
                }
                Some(ObjectType::Singleton { .. }) => singletons.push(object),
                Some(ObjectType::View { .. }) => {
                    return Err(SnapshotError::View(display_name(object).to_string()).into());
                }
                Some(ObjectType::Object { .. }) => match schema.get_object(object) {
                    Some(def) if def.is_multi_variant() => {
                        shards.extend(def.variants().iter().map(|v| ShardKey {
                            object,
                            variant: Some(v.name.as_str()),
                        }))
                    }
                    _ => shards.push(ShardKey {
                        object,
                        variant: None,
                    }),
                },
            }
        }
        let mut edges = Edges::build(schema, shards);
        let deferred = edges.break_cycles()?;
        let order = edges.topo_order();
        if order.len() != edges.nodes.len() {
            return Err(SnapshotError::UnbrokenCycle.into());
        }
        let shards = order.into_iter().filter_map(|i| edges.shard(i)).collect();
        Ok(ShardGraph {
            shards,
            singletons,
            deferred,
        })
    }

    pub fn deferred_from(&self, key: ShardKey<'s>) -> impl Iterator<Item = &Deferred<'s>> {
        self.deferred.iter().filter(move |d| d.from == key)
    }

    pub fn validate_static_refs(
        schema: &'s Schema,
        selection: &[&'s str],
        allow: &HashSet<&'s str>,
    ) -> Result<()> {
        Selection {
            schema,
            selected: selection.iter().copied().collect(),
        }
        .validate(selection, allow)
    }
}

struct Node<'s> {
    shard: ShardKey<'s>,
    out: BTreeMap<usize, EdgeFields<'s>>,
}

struct Edges<'s> {
    nodes: Vec<Node<'s>>,
}

impl<'s> Edges<'s> {
    fn build(schema: &'s Schema, shards: Vec<ShardKey<'s>>) -> Self {
        let index: HashMap<ShardKey<'s>, usize> =
            shards.iter().enumerate().map(|(i, s)| (*s, i)).collect();
        let mut nodes: Vec<Node<'s>> = shards
            .into_iter()
            .map(|shard| Node {
                shard,
                out: BTreeMap::new(),
            })
            .collect();
        let mut found = Vec::new();
        let mut visited = HashSet::new();
        for (i, node) in nodes.iter_mut().enumerate() {
            for (name, field) in ShardFields::of(schema, node.shard).properties() {
                let mutability = match field.update {
                    FieldUpdate::Mutable => Mutability::Mutable,
                    FieldUpdate::Immutable | FieldUpdate::ServerSet => Mutability::Fixed,
                };
                found.clear();
                visited.clear();
                Walker {
                    schema,
                    visited: &mut visited,
                    out: &mut found,
                }
                .field_type(&field.typ, Arity::One);
                for &(target, arity) in &found {
                    let Some(&j) = index.get(&target) else {
                        continue;
                    };
                    if j != i {
                        node.out.entry(j).or_default().add(EdgeField {
                            name,
                            mutability,
                            arity,
                        });
                    }
                }
            }
        }
        Edges { nodes }
    }

    fn shard(&self, i: usize) -> Option<ShardKey<'s>> {
        self.nodes.get(i).map(|n| n.shard)
    }

    fn out(&self, i: usize) -> impl Iterator<Item = (usize, &EdgeFields<'s>)> {
        self.nodes
            .get(i)
            .into_iter()
            .flat_map(|n| n.out.iter().map(|(&v, ef)| (v, ef)))
    }

    fn label(&self, i: usize) -> String {
        self.shard(i).map(|s| s.to_string()).unwrap_or_default()
    }

    fn topo_order(&self) -> Vec<usize> {
        let mut pending: Vec<usize> = self.nodes.iter().map(|n| n.out.len()).collect();
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        for (j, node) in self.nodes.iter().enumerate() {
            for &i in node.out.keys() {
                if let Some(d) = dependents.get_mut(i) {
                    d.push(j);
                }
            }
        }
        let mut order: Vec<usize> = Vec::with_capacity(self.nodes.len());
        let mut ready: Vec<usize> = pending
            .iter()
            .enumerate()
            .filter(|&(_, &p)| p == 0)
            .map(|(i, _)| i)
            .collect();
        while let Some(i) = ready.pop() {
            order.push(i);
            for &j in dependents.get(i).into_iter().flatten() {
                if let Some(p) = pending.get_mut(j) {
                    *p -= 1;
                    if *p == 0 {
                        ready.push(j);
                    }
                }
            }
        }
        order
    }

    fn break_cycles(&mut self) -> Result<Vec<Deferred<'s>>> {
        struct Candidate<'s> {
            u: usize,
            v: usize,
            field: &'s str,
            arity: Arity,
        }

        let mut deferred = Vec::new();
        loop {
            let mut progressed = false;
            for scc in self.sccs() {
                if let [v] = scc.as_slice()
                    && !self.out(*v).any(|(w, _)| w == *v)
                {
                    continue;
                }
                let members: HashSet<usize> = scc.iter().copied().collect();
                let mut sorted = scc.clone();
                sorted.sort_by_cached_key(|&i| self.label(i));

                let mut best: Option<Candidate<'s>> = None;
                for &u in &sorted {
                    for (v, ef) in self.out(u) {
                        if !members.contains(&v) {
                            continue;
                        }
                        if let Some(f) = ef.pick_mutable() {
                            let take = match &best {
                                None => true,
                                Some(b) => b.arity == Arity::One && f.arity == Arity::Many,
                            };
                            if take {
                                best = Some(Candidate {
                                    u,
                                    v,
                                    field: f.name,
                                    arity: f.arity,
                                });
                            }
                        }
                    }
                }
                let Some(Candidate { u, v, field, .. }) = best else {
                    let mut nodes: Vec<String> = scc.iter().map(|&i| self.label(i)).collect();
                    nodes.sort();
                    let hint = scc
                        .iter()
                        .flat_map(|&u| self.out(u).map(move |(v, ef)| (u, v, ef)))
                        .filter(|(_, v, _)| members.contains(v))
                        .find_map(|(u, v, ef)| {
                            ef.any_fixed().map(|name| CycleHint::Immutable {
                                from: self.label(u),
                                field: name.to_string(),
                                to: self.label(v),
                            })
                        })
                        .unwrap_or(CycleHint::None);
                    return Err(SnapshotError::Cycle {
                        nodes: nodes.join(", "),
                        hint,
                    }
                    .into());
                };
                if let Some(node) = self.nodes.get_mut(u)
                    && let Some(ef) = node.out.get_mut(&v)
                {
                    ef.fields.retain(|f| f.name != field);
                    if ef.fields.is_empty() {
                        node.out.remove(&v);
                    }
                }
                if let (Some(from), Some(to)) = (self.shard(u), self.shard(v)) {
                    deferred.push(Deferred { from, to, field });
                }
                progressed = true;
            }
            if !progressed {
                return Ok(deferred);
            }
        }
    }

    fn sccs(&self) -> Vec<Vec<usize>> {
        let mut tarjan = Tarjan {
            visits: vec![Visit::default(); self.nodes.len()],
            stack: Vec::new(),
            next: 0,
            sccs: Vec::new(),
        };
        let mut work: Vec<(usize, Keys<'_, usize, EdgeFields<'s>>)> = Vec::new();
        for (root, node) in self.nodes.iter().enumerate() {
            if !tarjan.enter(root) {
                continue;
            }
            work.push((root, node.out.keys()));
            while let Some((u, targets)) = work.last_mut() {
                let u = *u;
                if let Some(&w) = targets.next() {
                    if tarjan.enter(w) {
                        if let Some(next) = self.nodes.get(w) {
                            work.push((w, next.out.keys()));
                        }
                    } else {
                        tarjan.reach(u, w);
                    }
                    continue;
                }
                work.pop();
                tarjan.leave(u, work.last().map(|&(parent, _)| parent));
            }
        }
        tarjan.sccs
    }
}

#[derive(Clone, Copy, Default)]
struct Visit {
    index: Option<u32>,
    lowlink: u32,
    on_stack: bool,
}

struct Tarjan {
    visits: Vec<Visit>,
    stack: Vec<usize>,
    next: u32,
    sccs: Vec<Vec<usize>>,
}

impl Tarjan {
    fn enter(&mut self, v: usize) -> bool {
        let Some(visit) = self.visits.get_mut(v).filter(|visit| visit.index.is_none()) else {
            return false;
        };
        *visit = Visit {
            index: Some(self.next),
            lowlink: self.next,
            on_stack: true,
        };
        self.next += 1;
        self.stack.push(v);
        true
    }

    fn lower(&mut self, v: usize, to: u32) {
        if let Some(visit) = self.visits.get_mut(v) {
            visit.lowlink = visit.lowlink.min(to);
        }
    }

    fn reach(&mut self, u: usize, w: usize) {
        if let Some(&Visit {
            index: Some(index),
            on_stack: true,
            ..
        }) = self.visits.get(w)
        {
            self.lower(u, index);
        }
    }

    fn leave(&mut self, u: usize, parent: Option<usize>) {
        let Some(&Visit { index, lowlink, .. }) = self.visits.get(u) else {
            return;
        };
        if let Some(parent) = parent {
            self.lower(parent, lowlink);
        }
        if index != Some(lowlink) {
            return;
        }
        let mut component = Vec::new();
        while let Some(node) = self.stack.pop() {
            if let Some(visit) = self.visits.get_mut(node) {
                visit.on_stack = false;
            }
            component.push(node);
            if node == u {
                break;
            }
        }
        self.sccs.push(component);
    }
}

struct Selection<'s> {
    schema: &'s Schema,
    selected: HashSet<&'s str>,
}

impl<'s> Selection<'s> {
    fn validate(&self, selection: &[&'s str], allow: &HashSet<&'s str>) -> Result<()> {
        let mut found = Vec::new();
        let mut visited = HashSet::new();
        for &object in selection {
            found.clear();
            visited.clear();
            Walker {
                schema: self.schema,
                visited: &mut visited,
                out: &mut found,
            }
            .object(object, Arity::One);
            for &(target, _) in &found {
                if target.object == object
                    || self.selected.contains(target.object)
                    || allow.contains(target.object)
                {
                    continue;
                }
                let to = display_name(target.object).to_string();
                let remedy = if self.creates_cycle(object, target.object) {
                    Remedy::AllowUnresolved(to.clone())
                } else {
                    Remedy::Select(to.clone())
                };
                return Err(SnapshotError::UnselectedReference {
                    from: display_name(object).to_string(),
                    to,
                    remedy,
                }
                .into());
            }
        }
        Ok(())
    }

    fn refs(&self, object: &str) -> Vec<&'s str> {
        let mut out = Vec::new();
        Walker {
            schema: self.schema,
            visited: &mut HashSet::new(),
            out: &mut out,
        }
        .object(object, Arity::One);
        let mut objects: Vec<&'s str> = out.into_iter().map(|(k, _)| k.object).collect();
        objects.sort_unstable();
        objects.dedup();
        objects
    }

    fn creates_cycle(&self, from: &'s str, target: &'s str) -> bool {
        let mut universe: HashSet<&str> = self.selected.clone();
        universe.insert(target);
        universe.insert(from);
        let adjacency: HashMap<&str, Vec<&str>> = universe
            .iter()
            .map(|&name| {
                let outgoing = self
                    .refs(name)
                    .into_iter()
                    .filter(|t| universe.contains(t) && *t != name)
                    .collect();
                (name, outgoing)
            })
            .collect();
        let mut stack = vec![target];
        let mut seen: HashSet<&str> = HashSet::new();
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            for &n in adjacency.get(node).into_iter().flatten() {
                if n == target {
                    return true;
                }
                stack.push(n);
            }
        }
        false
    }
}
