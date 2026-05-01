/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::context::Context;
use crate::app::error::{CliError, CliResult};
use crate::cli::SnapshotArgs;
use crate::jmap::Jmap;
use crate::jmap::protocol::check_response;
use crate::schema::resolve;
use crate::schema::{
    FieldType, FieldUpdate, Fields, MapValueType, ObjectSchema, ObjectType, ScalarType, Schema,
    StringFormat,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

struct Ctx<'a> {
    jmap: &'a Jmap<'a>,
    schema: &'a Schema,
    allow: &'a HashSet<String>,
    include_secrets: bool,
    limit: usize,
}

pub fn run(ctx: &Context, args: &SnapshotArgs) -> CliResult<()> {
    let selection = resolve_selection(&ctx.schema, &args.objects)?;
    let allow_unresolved = canonicalise_allow(&ctx.schema, &args.allow_unresolved)?;

    validate_static_refs(&ctx.schema, &selection, &allow_unresolved)?;

    let plan = build_plan(&ctx.schema, &selection)?;

    let mut reporter = Reporter::new(args.quiet);
    reporter.plan_header(&plan);

    let mut sink = open_output(args.output.as_deref())?;

    if !args.no_destroys {
        let mut emitted: HashSet<String> = HashSet::new();
        for shard in plan.iter_non_singletons() {
            if emitted.insert(shard.key.canonical.clone()) {
                emit_destroy(&mut sink, &shard.key.canonical)?;
            }
        }
    }

    let jmap = Jmap::new(&ctx.client, &ctx.session.api_path);
    let snap_ctx = Ctx {
        jmap: &jmap,
        schema: &ctx.schema,
        allow: &allow_unresolved,
        include_secrets: args.include_secrets,
        limit: ctx.session.max_objects_in_get.max(1),
    };

    let mut cache = FetchCache::new();
    for shard in plan.iter_non_singletons() {
        let deferred = plan.deferred_for(&shard.key);
        emit_create(&mut sink, &snap_ctx, shard, &deferred, &mut cache, &mut reporter)?;
    }

    emit_deferred_updates(&mut sink, &snap_ctx, &plan, &mut cache)?;

    for name in plan.singletons() {
        emit_singleton_update(&mut sink, &snap_ctx, name, &mut reporter)?;
    }

    sink.flush()?;
    reporter.done();
    Ok(())
}

fn open_output(path: Option<&Path>) -> CliResult<Box<dyn Write>> {
    match path {
        Some(p) => {
            let f = File::create(p)?;
            Ok(Box::new(BufWriter::new(f)))
        }
        None => Ok(Box::new(BufWriter::new(std::io::stdout().lock()))),
    }
}

fn resolve_selection(schema: &Schema, raws: &[String]) -> CliResult<Vec<String>> {
    let mut out = Vec::with_capacity(raws.len());
    let mut seen: HashSet<String> = HashSet::new();
    for raw in raws {
        if raw.contains('/') {
            return Err(CliError::msg(format!(
                "snapshot cannot select variants individually ({}); use the bare object name",
                raw
            )));
        }
        let canonical = resolve::require_object(schema, raw)?;
        if !seen.insert(canonical.to_string()) {
            continue;
        }
        out.push(canonical.to_string());
    }
    Ok(out)
}

fn canonicalise_allow(schema: &Schema, raws: &[String]) -> CliResult<HashSet<String>> {
    let mut out = HashSet::with_capacity(raws.len());
    for raw in raws {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let canonical = resolve::require_object(schema, raw)?;
        out.insert(canonical.to_string());
    }
    Ok(out)
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct ShardKey {
    canonical: String,
    variant: Option<String>,
}

struct Shard {
    key: ShardKey,
    is_singleton: bool,
}

struct Plan {
    shards: Vec<Shard>,
    singletons: Vec<String>,
    deferred: Vec<DeferredEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeferredEdge {
    from_shard: ShardKey,
    to_shard: ShardKey,
    field: String,
}

impl Plan {
    fn iter_non_singletons(&self) -> impl Iterator<Item = &Shard> {
        self.shards.iter().filter(|s| !s.is_singleton)
    }
    fn singletons(&self) -> &[String] {
        &self.singletons
    }
    fn deferred_for(&self, key: &ShardKey) -> Vec<&DeferredEdge> {
        self.deferred
            .iter()
            .filter(|d| &d.from_shard == key)
            .collect()
    }
}

fn build_plan(schema: &Schema, selection: &[String]) -> CliResult<Plan> {
    let mut shards: Vec<Shard> = Vec::new();
    let mut singletons: Vec<String> = Vec::new();

    for canonical in selection {
        let obj = schema.objects.get(canonical).ok_or_else(|| {
            CliError::UnexpectedResponse(format!("schema missing entry for {canonical}"))
        })?;
        match obj {
            ObjectType::Singleton { .. } => {
                singletons.push(canonical.clone());
            }
            ObjectType::Object { .. } => {
                let obj_schema = schema.schemas.get(canonical);
                match obj_schema {
                    Some(ObjectSchema::Single { .. }) | None => shards.push(Shard {
                        key: ShardKey {
                            canonical: canonical.clone(),
                            variant: None,
                        },
                        is_singleton: false,
                    }),
                    Some(ObjectSchema::Multiple { variants }) => {
                        for v in variants {
                            shards.push(Shard {
                                key: ShardKey {
                                    canonical: canonical.clone(),
                                    variant: Some(v.name.clone()),
                                },
                                is_singleton: false,
                            });
                        }
                    }
                }
            }
            ObjectType::View { .. } => {
                return Err(CliError::msg(format!(
                    "{} is a view and cannot be snapshotted directly",
                    resolve::display_name(canonical)
                )));
            }
        }
    }

    let deferred = topologically_sort(schema, &mut shards)?;
    Ok(Plan {
        shards,
        singletons,
        deferred,
    })
}

fn topologically_sort(schema: &Schema, shards: &mut Vec<Shard>) -> CliResult<Vec<DeferredEdge>> {
    let index: HashMap<ShardKey, usize> = shards
        .iter()
        .enumerate()
        .map(|(i, s)| (s.key.clone(), i))
        .collect();

    let n = shards.len();
    let mut edges: Vec<BTreeMap<usize, EdgeFields>> = (0..n).map(|_| BTreeMap::new()).collect();
    for (i, shard) in shards.iter().enumerate() {
        let Some(fields) = shard_fields(schema, &shard.key) else {
            continue;
        };
        let mut top_refs: Vec<TopRef> = Vec::new();
        collect_top_refs(schema, fields, &mut top_refs);
        for tr in top_refs {
            let Some(&j) = index.get(&tr.target) else {
                continue;
            };
            if j == i {
                continue;
            }
            edges[i]
                .entry(j)
                .or_default()
                .add(tr.field, tr.mutable, tr.multi);
        }
    }

    let deferred = break_cycles(shards, &mut edges)?;
    let order = topo_order(&edges, n);
    if order.len() != n {
        return Err(CliError::msg(
            "cannot snapshot: dependency graph still has cycles after cycle-breaking",
        ));
    }

    let mut taken: Vec<Option<Shard>> = shards.drain(..).map(Some).collect();
    for i in order {
        if let Some(s) = taken[i].take() {
            shards.push(s);
        }
    }
    Ok(deferred)
}

#[derive(Default, Debug, Clone)]
struct EdgeFields {
    fields: Vec<EdgeField>,
}

#[derive(Debug, Clone)]
struct EdgeField {
    name: String,
    mutable: bool,
    multi: bool,
}

impl EdgeFields {
    fn add(&mut self, name: String, mutable: bool, multi: bool) {
        if !self.fields.iter().any(|f| f.name == name) {
            self.fields.push(EdgeField {
                name,
                mutable,
                multi,
            });
        }
    }
    fn pick_mutable(&self) -> Option<&EdgeField> {
        let mut chosen: Option<&EdgeField> = None;
        for f in &self.fields {
            if !f.mutable {
                continue;
            }
            match &chosen {
                None => chosen = Some(f),
                Some(c) if !c.multi && f.multi => chosen = Some(f),
                _ => {}
            }
        }
        chosen
    }
    fn any_immutable(&self) -> Option<&str> {
        self.fields
            .iter()
            .find(|f| !f.mutable)
            .map(|f| f.name.as_str())
    }
}

struct TopRef {
    field: String,
    mutable: bool,
    multi: bool,
    target: ShardKey,
}

fn collect_top_refs(schema: &Schema, fields: &Fields, out: &mut Vec<TopRef>) {
    for (name, field) in &fields.properties {
        let mutable = matches!(field.update, FieldUpdate::Mutable);
        let mut targets: Vec<(ShardKey, bool)> = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        collect_ft_refs_multi(schema, &field.typ, false, &mut targets, &mut visited);
        for (target, multi) in targets {
            out.push(TopRef {
                field: name.clone(),
                mutable,
                multi,
                target,
            });
        }
    }
}

fn collect_ft_refs_multi(
    schema: &Schema,
    t: &FieldType,
    multi: bool,
    out: &mut Vec<(ShardKey, bool)>,
    visited: &mut HashSet<String>,
) {
    match t {
        FieldType::ObjectId { object_name, .. } => push_shard_refs_multi(schema, object_name, multi, out),
        FieldType::Set {
            class: ScalarType::ObjectId { object_name },
            ..
        } => push_shard_refs_multi(schema, object_name, true, out),
        FieldType::Map {
            key_class,
            value_class,
            ..
        } => {
            if let ScalarType::ObjectId { object_name } = key_class {
                push_shard_refs_multi(schema, object_name, true, out);
            }
            if let MapValueType::Object { object_name } = value_class {
                recurse_embedded_multi(schema, object_name, true, out, visited);
            }
        }
        FieldType::Object { object_name, .. } => {
            recurse_embedded_multi(schema, object_name, multi, out, visited);
        }
        FieldType::ObjectList { object_name, .. } => {
            recurse_embedded_multi(schema, object_name, true, out, visited);
        }
        _ => {}
    }
}

fn push_shard_refs_multi(
    schema: &Schema,
    object_name: &str,
    multi: bool,
    out: &mut Vec<(ShardKey, bool)>,
) {
    let mut tmp: Vec<ShardKey> = Vec::new();
    push_shard_refs(schema, object_name, &mut tmp);
    for k in tmp {
        out.push((k, multi));
    }
}

fn recurse_embedded_multi(
    schema: &Schema,
    object_name: &str,
    multi: bool,
    out: &mut Vec<(ShardKey, bool)>,
    visited: &mut HashSet<String>,
) {
    if !visited.insert(object_name.to_string()) {
        return;
    }
    let Some(obj_schema) = schema.schemas.get(object_name) else {
        return;
    };
    match obj_schema {
        ObjectSchema::Single { schema_name } => {
            if let Some(f) = schema.fields.get(schema_name) {
                for fld in f.properties.values() {
                    collect_ft_refs_multi(schema, &fld.typ, multi, out, visited);
                }
            }
        }
        ObjectSchema::Multiple { variants } => {
            for v in variants {
                if let Some(sn) = &v.schema_name
                    && let Some(f) = schema.fields.get(sn)
                {
                    for fld in f.properties.values() {
                        collect_ft_refs_multi(schema, &fld.typ, multi, out, visited);
                    }
                }
            }
        }
    }
}

fn topo_order(edges: &[BTreeMap<usize, EdgeFields>], n: usize) -> Vec<usize> {
    let mut deps: Vec<HashSet<usize>> = (0..n).map(|i| edges[i].keys().copied().collect()).collect();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut ready: Vec<usize> = (0..n).filter(|&i| deps[i].is_empty()).collect();
    while let Some(i) = ready.pop() {
        order.push(i);
        for (j, dep) in deps.iter_mut().enumerate() {
            if dep.remove(&i) && dep.is_empty() {
                ready.push(j);
            }
        }
    }
    order
}

fn break_cycles(
    shards: &[Shard],
    edges: &mut [BTreeMap<usize, EdgeFields>],
) -> CliResult<Vec<DeferredEdge>> {
    let mut deferred: Vec<DeferredEdge> = Vec::new();
    loop {
        let sccs = tarjan_sccs(edges);
        let mut progressed = false;
        for scc in sccs {
            if scc.len() == 1 {
                let v = scc[0];
                if !edges[v].contains_key(&v) {
                    continue;
                }
            }
            let scc_set: HashSet<usize> = scc.iter().copied().collect();
            let mut sorted_scc: Vec<usize> = scc.to_vec();
            sorted_scc.sort_by(|&a, &b| {
                display_shard(&shards[a].key).cmp(&display_shard(&shards[b].key))
            });

            #[derive(Clone)]
            struct Candidate {
                u: usize,
                v: usize,
                field: String,
                multi: bool,
            }
            let mut best: Option<Candidate> = None;
            for &u in &sorted_scc {
                let Some(targets) = edges.get(u) else { continue };
                for (&v, ef) in targets.iter() {
                    if !scc_set.contains(&v) {
                        continue;
                    }
                    if let Some(f) = ef.pick_mutable() {
                        let cand = Candidate {
                            u,
                            v,
                            field: f.name.clone(),
                            multi: f.multi,
                        };
                        let take = match &best {
                            None => true,
                            Some(b) => !b.multi && cand.multi,
                        };
                        if take {
                            best = Some(cand);
                        }
                    }
                }
            }
            let chosen = best.map(|c| (c.u, c.v, c.field));
            let Some((u, v, field)) = chosen else {
                let mut nodes: Vec<String> = scc
                    .iter()
                    .map(|&i| display_shard(&shards[i].key))
                    .collect();
                nodes.sort();
                let mut immutable_hint: Option<String> = None;
                'find_imm: for &u in &scc {
                    let Some(targets) = edges.get(u) else {
                        continue;
                    };
                    for (&v, ef) in targets.iter() {
                        if !scc_set.contains(&v) {
                            continue;
                        }
                        if let Some(name) = ef.any_immutable() {
                            immutable_hint = Some(format!(
                                "{}.{} -> {}",
                                display_shard(&shards[u].key),
                                name,
                                display_shard(&shards[v].key)
                            ));
                            break 'find_imm;
                        }
                    }
                }
                let mut msg = String::from(
                    "cannot snapshot: cyclic dependency between selected types: ",
                );
                msg.push_str(&nodes.join(", "));
                if let Some(h) = immutable_hint {
                    msg.push_str(" (immutable field closes the cycle: ");
                    msg.push_str(&h);
                    msg.push(')');
                }
                return Err(CliError::msg(msg));
            };
            if let Some(targets) = edges.get_mut(u)
                && let Some(ef) = targets.get_mut(&v)
            {
                ef.fields.retain(|f| f.name != field);
                if ef.fields.is_empty() {
                    targets.remove(&v);
                }
            }
            deferred.push(DeferredEdge {
                from_shard: shards[u].key.clone(),
                to_shard: shards[v].key.clone(),
                field,
            });
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    Ok(deferred)
}

fn tarjan_sccs(edges: &[BTreeMap<usize, EdgeFields>]) -> Vec<Vec<usize>> {
    let n = edges.len();
    let adj: Vec<Vec<usize>> = edges.iter().map(|m| m.keys().copied().collect()).collect();

    let mut indices: Vec<i64> = vec![-1; n];
    let mut lowlink: Vec<usize> = vec![0; n];
    let mut on_stack: Vec<bool> = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next_index: usize = 0;
    let mut sccs: Vec<Vec<usize>> = Vec::new();

    for v in 0..n {
        if indices[v] >= 0 {
            continue;
        }
        let mut work: Vec<(usize, usize)> = Vec::new();
        indices[v] = next_index as i64;
        lowlink[v] = next_index;
        next_index += 1;
        stack.push(v);
        on_stack[v] = true;
        work.push((v, 0));
        while let Some((u, i)) = work.last().copied() {
            if i < adj[u].len() {
                let w = adj[u][i];
                if let Some(slot) = work.last_mut() {
                    slot.1 = i + 1;
                }
                if indices[w] < 0 {
                    indices[w] = next_index as i64;
                    lowlink[w] = next_index;
                    next_index += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    let wi = indices[w] as usize;
                    if wi < lowlink[u] {
                        lowlink[u] = wi;
                    }
                }
            } else {
                work.pop();
                if let Some(&(parent, _)) = work.last()
                    && lowlink[u] < lowlink[parent]
                {
                    lowlink[parent] = lowlink[u];
                }
                if (lowlink[u] as i64) == indices[u] {
                    let mut comp: Vec<usize> = Vec::new();
                    while let Some(node) = stack.pop() {
                        on_stack[node] = false;
                        comp.push(node);
                        if node == u {
                            break;
                        }
                    }
                    sccs.push(comp);
                }
            }
        }
    }
    sccs
}

fn display_shard(key: &ShardKey) -> String {
    match &key.variant {
        Some(v) => format!("{}/{}", resolve::display_name(&key.canonical), v),
        None => resolve::display_name(&key.canonical).to_string(),
    }
}

fn shard_fields<'a>(schema: &'a Schema, key: &ShardKey) -> Option<&'a Fields> {
    let obj_schema = schema.schemas.get(&key.canonical)?;
    match obj_schema {
        ObjectSchema::Single { schema_name } => schema.fields.get(schema_name),
        ObjectSchema::Multiple { variants } => {
            let v = key
                .variant
                .as_ref()
                .and_then(|name| variants.iter().find(|v| &v.name == name))?;
            schema.fields.get(v.schema_name.as_ref()?)
        }
    }
}

fn collect_shard_refs(
    schema: &Schema,
    fields: &Fields,
    out: &mut Vec<ShardKey>,
    visited: &mut HashSet<String>,
) {
    for field in fields.properties.values() {
        collect_ft_refs(schema, &field.typ, out, visited);
    }
}

fn collect_ft_refs(
    schema: &Schema,
    t: &FieldType,
    out: &mut Vec<ShardKey>,
    visited: &mut HashSet<String>,
) {
    match t {
        FieldType::ObjectId { object_name, .. } => push_shard_refs(schema, object_name, out),
        FieldType::Set {
            class: ScalarType::ObjectId { object_name },
            ..
        } => push_shard_refs(schema, object_name, out),
        FieldType::Map {
            key_class,
            value_class,
            ..
        } => {
            if let ScalarType::ObjectId { object_name } = key_class {
                push_shard_refs(schema, object_name, out);
            }
            if let MapValueType::Object { object_name } = value_class {
                recurse_embedded(schema, object_name, out, visited);
            }
        }
        FieldType::Object { object_name, .. } => {
            recurse_embedded(schema, object_name, out, visited);
        }
        FieldType::ObjectList { object_name, .. } => {
            recurse_embedded(schema, object_name, out, visited);
        }
        _ => {}
    }
}

fn push_shard_refs(schema: &Schema, object_name: &str, out: &mut Vec<ShardKey>) {
    let (canonical, variant) = split_view(object_name);
    let entry = schema.objects.get(canonical);
    match entry {
        Some(ObjectType::View {
            object_name: parent,
        }) => {
            let parent_variant = object_name.rsplit_once('/').map(|(_, v)| v.to_string());
            out.push(ShardKey {
                canonical: parent.clone(),
                variant: parent_variant,
            });
        }
        _ => match schema.schemas.get(canonical) {
            Some(ObjectSchema::Multiple { variants }) => {
                if let Some(v) = variant {
                    out.push(ShardKey {
                        canonical: canonical.to_string(),
                        variant: Some(v.to_string()),
                    });
                } else {
                    for v in variants {
                        out.push(ShardKey {
                            canonical: canonical.to_string(),
                            variant: Some(v.name.clone()),
                        });
                    }
                }
            }
            _ => out.push(ShardKey {
                canonical: canonical.to_string(),
                variant: None,
            }),
        },
    }
}

fn split_view(object_name: &str) -> (&str, Option<&str>) {
    match object_name.split_once('/') {
        Some((base, variant)) => (base, Some(variant)),
        None => (object_name, None),
    }
}

fn recurse_embedded(
    schema: &Schema,
    object_name: &str,
    out: &mut Vec<ShardKey>,
    visited: &mut HashSet<String>,
) {
    if !visited.insert(object_name.to_string()) {
        return;
    }
    let Some(obj_schema) = schema.schemas.get(object_name) else {
        return;
    };
    match obj_schema {
        ObjectSchema::Single { schema_name } => {
            if let Some(f) = schema.fields.get(schema_name) {
                collect_shard_refs(schema, f, out, visited);
            }
        }
        ObjectSchema::Multiple { variants } => {
            for v in variants {
                if let Some(sn) = &v.schema_name
                    && let Some(f) = schema.fields.get(sn)
                {
                    collect_shard_refs(schema, f, out, visited);
                }
            }
        }
    }
}

fn validate_static_refs(
    schema: &Schema,
    selection: &[String],
    allow: &HashSet<String>,
) -> CliResult<()> {
    let selected: HashSet<&str> = selection.iter().map(String::as_str).collect();
    let allow_refs: HashSet<&str> = allow.iter().map(String::as_str).collect();

    for canonical in selection {
        let obj_schema = schema.schemas.get(canonical);
        let variants_fields: Vec<&Fields> = match obj_schema {
            Some(ObjectSchema::Single { schema_name }) => {
                schema.fields.get(schema_name).into_iter().collect()
            }
            Some(ObjectSchema::Multiple { variants }) => variants
                .iter()
                .filter_map(|v| v.schema_name.as_ref().and_then(|n| schema.fields.get(n)))
                .collect(),
            None => Vec::new(),
        };
        for fields in variants_fields {
            let mut refs: Vec<ShardKey> = Vec::new();
            let mut visited: HashSet<String> = HashSet::new();
            collect_shard_refs(schema, fields, &mut refs, &mut visited);
            for r in &refs {
                if r.canonical == *canonical {
                    continue;
                }
                if selected.contains(r.canonical.as_str())
                    || allow_refs.contains(r.canonical.as_str())
                {
                    continue;
                }
                let from = resolve::display_name(canonical);
                let to = resolve::display_name(&r.canonical);
                let creates_cycle =
                    adding_target_creates_cycle(schema, &selected, canonical, &r.canonical);
                let recommendation = if creates_cycle {
                    format!(
                        "adding {to} to the selection would form a cycle, so use \
                         --allow-unresolved {to}"
                    )
                } else {
                    format!("add {to} to the selection, or use --allow-unresolved {to}")
                };
                return Err(CliError::msg(format!(
                    "{from} references {to} but {to} is not in the snapshot selection; \
                     {recommendation}"
                )));
            }
        }
    }
    Ok(())
}

fn adding_target_creates_cycle(
    schema: &Schema,
    selected: &HashSet<&str>,
    from: &str,
    target: &str,
) -> bool {
    let mut universe: Vec<String> = selected.iter().map(|s| s.to_string()).collect();
    if !universe.iter().any(|s| s == target) {
        universe.push(target.to_string());
    }
    if !universe.iter().any(|s| s == from) {
        universe.push(from.to_string());
    }
    let universe_set: HashSet<&str> = universe.iter().map(String::as_str).collect();
    let mut adj: HashMap<String, HashSet<String>> = HashMap::new();
    for name in &universe {
        let outgoing = canonical_refs(schema, name);
        let entry = adj.entry(name.clone()).or_default();
        for tgt in outgoing {
            if universe_set.contains(tgt.as_str()) && tgt != *name {
                entry.insert(tgt);
            }
        }
    }
    let mut stack = vec![target.to_string()];
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(node) = stack.pop() {
        if !seen.insert(node.clone()) {
            continue;
        }
        if let Some(neighbors) = adj.get(&node) {
            for n in neighbors {
                if n == target {
                    return true;
                }
                stack.push(n.clone());
            }
        }
    }
    false
}

fn canonical_refs(schema: &Schema, canonical: &str) -> Vec<String> {
    let mut out: Vec<ShardKey> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let obj_schema = schema.schemas.get(canonical);
    match obj_schema {
        Some(ObjectSchema::Single { schema_name }) => {
            if let Some(f) = schema.fields.get(schema_name) {
                collect_shard_refs(schema, f, &mut out, &mut visited);
            }
        }
        Some(ObjectSchema::Multiple { variants }) => {
            for v in variants {
                if let Some(sn) = v.schema_name.as_ref()
                    && let Some(f) = schema.fields.get(sn)
                {
                    collect_shard_refs(schema, f, &mut out, &mut visited);
                }
            }
        }
        None => {}
    }
    let mut canonicals: Vec<String> = out.into_iter().map(|k| k.canonical).collect();
    canonicals.sort();
    canonicals.dedup();
    canonicals
}

fn emit_destroy<W: Write>(sink: &mut W, canonical: &str) -> CliResult<()> {
    sink.write_all(b"{\"@type\":\"destroy\",\"object\":\"")?;
    sink.write_all(resolve::display_name(canonical).as_bytes())?;
    sink.write_all(b"\"}\n")?;
    Ok(())
}

fn emit_create<W: Write>(
    sink: &mut W,
    cx: &Ctx<'_>,
    shard: &Shard,
    deferred: &[&DeferredEdge],
    cache: &mut FetchCache,
    reporter: &mut Reporter,
) -> CliResult<()> {
    let schema = cx.schema;
    let allow = cx.allow;
    let include_secrets = cx.include_secrets;
    let fields = shard_fields(schema, &shard.key).ok_or_else(|| {
        CliError::UnexpectedResponse(format!("no schema available for {}", shard.key.canonical))
    })?;

    cache.ensure(cx, &shard.key.canonical, reporter)?;

    let objs = cache.objects_for(&shard.key);
    if objs.is_empty() {
        reporter.shard_done(&shard.key, 0);
        return Ok(());
    }

    let omit: HashSet<&str> = deferred.iter().map(|d| d.field.as_str()).collect();

    sink.write_all(b"{\"@type\":\"create\",\"object\":\"")?;
    sink.write_all(resolve::display_name(&shard.key.canonical).as_bytes())?;
    sink.write_all(b"\",\"value\":{")?;

    let mut first_entry = true;
    let mut total = 0usize;
    for obj in objs {
        let server_id = match obj.get("id").and_then(Value::as_str) {
            Some(s) => s,
            None => continue,
        };
        let mut out_obj = transform_object(schema, fields, obj, allow, include_secrets);
        if shard.key.variant.is_some()
            && let Some(at_type) = obj.get("@type")
        {
            out_obj.insert("@type".into(), at_type.clone());
        }
        for k in &omit {
            out_obj.remove(*k);
        }

        if !first_entry {
            sink.write_all(b",")?;
        }
        first_entry = false;
        sink.write_all(b"\"")?;
        write_client_id(sink, &shard.key.canonical, server_id)?;
        sink.write_all(b"\":")?;
        serde_json::to_writer(&mut *sink, &Value::Object(out_obj))?;
        total += 1;
    }

    sink.write_all(b"}}\n")?;
    reporter.shard_done(&shard.key, total);
    Ok(())
}

fn emit_deferred_updates<W: Write>(
    sink: &mut W,
    cx: &Ctx<'_>,
    plan: &Plan,
    cache: &mut FetchCache,
) -> CliResult<()> {
    let mut by_shard: Vec<(&ShardKey, Vec<&DeferredEdge>)> = Vec::new();
    for edge in &plan.deferred {
        if let Some(slot) = by_shard.iter_mut().find(|(k, _)| *k == &edge.from_shard) {
            slot.1.push(edge);
        } else {
            by_shard.push((&edge.from_shard, vec![edge]));
        }
    }
    for (key, edges) in by_shard {
        let Some(fields) = shard_fields(cx.schema, key) else {
            continue;
        };
        let objs = cache.objects_for(key);
        if objs.is_empty() {
            continue;
        }
        let field_names: HashSet<&str> = edges.iter().map(|e| e.field.as_str()).collect();
        for obj in objs {
            let Some(server_id) = obj.get("id").and_then(Value::as_str) else {
                continue;
            };
            let mut patch = Map::new();
            let transformed =
                transform_object(cx.schema, fields, obj, cx.allow, cx.include_secrets);
            for name in &field_names {
                if let Some(v) = transformed.get(*name) {
                    patch.insert((*name).to_string(), v.clone());
                }
            }
            if patch.is_empty() {
                continue;
            }
            sink.write_all(b"{\"@type\":\"update\",\"object\":\"")?;
            sink.write_all(resolve::display_name(&key.canonical).as_bytes())?;
            sink.write_all(b"\",\"id\":\"#")?;
            write_client_id(sink, &key.canonical, server_id)?;
            sink.write_all(b"\",\"value\":")?;
            serde_json::to_writer(&mut *sink, &Value::Object(patch))?;
            sink.write_all(b"}\n")?;
        }
    }
    Ok(())
}

type VariantGroups = HashMap<Option<String>, Vec<Map<String, Value>>>;

struct FetchCache {
    by_canonical: HashMap<String, VariantGroups>,
}

impl FetchCache {
    fn new() -> Self {
        FetchCache {
            by_canonical: HashMap::new(),
        }
    }

    fn ensure(
        &mut self,
        cx: &Ctx<'_>,
        canonical: &str,
        reporter: &mut Reporter,
    ) -> CliResult<()> {
        if self.by_canonical.contains_key(canonical) {
            return Ok(());
        }
        reporter.fetch_start(canonical);
        let mut groups: VariantGroups = HashMap::new();
        let total = fetch_all_partitioned(cx, canonical, &mut groups)?;
        reporter.fetch_done(total);
        self.by_canonical.insert(canonical.to_string(), groups);
        Ok(())
    }

    fn objects_for(&self, key: &ShardKey) -> &[Map<String, Value>] {
        self.by_canonical
            .get(&key.canonical)
            .and_then(|groups| groups.get(&key.variant))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

fn fetch_all_partitioned(
    cx: &Ctx<'_>,
    canonical: &str,
    groups: &mut VariantGroups,
) -> CliResult<usize> {
    let query_method = format!("{canonical}/query");
    let get_method = format!("{canonical}/get");
    let limit = cx.limit;
    let mut anchor: Option<String> = None;
    let mut total = 0usize;

    loop {
        let mut q_args = Map::new();
        q_args.insert("filter".into(), json!({}));
        q_args.insert("limit".into(), Value::from(limit));
        if let Some(a) = &anchor {
            q_args.insert("anchor".into(), Value::String(a.clone()));
            q_args.insert("anchorOffset".into(), Value::from(1));
        }
        let get_args = json!({
            "#ids": { "resultOf": "q", "name": query_method, "path": "/ids" },
            "properties": Value::Null,
        });

        let resp = cx.jmap.call_with(
            vec![
                (query_method.clone(), Value::Object(q_args), "q".into()),
                (get_method.clone(), get_args, "g".into()),
            ],
            None,
        )?;
        if resp.responses.len() != 2 {
            return Err(CliError::UnexpectedResponse(format!(
                "expected 2 responses, got {}",
                resp.responses.len()
            )));
        }
        let mut iter = resp.responses.into_iter();
        let q_resp = iter
            .next()
            .ok_or_else(|| CliError::UnexpectedResponse("missing query response".into()))?;
        let g_resp = iter
            .next()
            .ok_or_else(|| CliError::UnexpectedResponse("missing get response".into()))?;
        check_response(q_resp, &query_method)?;
        let g_result = check_response(g_resp, &get_method)?;

        let Some(list) = g_result.get("list").and_then(Value::as_array) else {
            break;
        };
        if list.is_empty() {
            break;
        }
        let last_id = list
            .last()
            .and_then(|v| v.get("id").and_then(Value::as_str))
            .map(String::from);
        let returned = list.len();

        partition_into(list, groups);
        total += returned;

        if returned < limit {
            break;
        }
        match last_id {
            Some(a) => anchor = Some(a),
            None => break,
        }
    }
    Ok(total)
}

fn partition_into(list: &[Value], groups: &mut VariantGroups) {
    for item in list {
        let Some(obj) = item.as_object() else { continue };
        let variant = obj.get("@type").and_then(Value::as_str).map(String::from);
        groups.entry(variant).or_default().push(obj.clone());
    }
}

fn emit_singleton_update<W: Write>(
    sink: &mut W,
    cx: &Ctx<'_>,
    canonical: &str,
    reporter: &mut Reporter,
) -> CliResult<()> {
    let jmap = cx.jmap;
    let schema = cx.schema;
    let allow = cx.allow;
    let include_secrets = cx.include_secrets;
    let fields = schema
        .schemas
        .get(canonical)
        .and_then(|s| match s {
            ObjectSchema::Single { schema_name } => schema.fields.get(schema_name),
            ObjectSchema::Multiple { .. } => None,
        })
        .ok_or_else(|| {
            CliError::UnexpectedResponse(format!("singleton schema missing for {canonical}"))
        })?;

    reporter.singleton_start(canonical);

    let method = format!("{canonical}/get");
    let result = jmap.call(
        &method,
        json!({ "ids": ["singleton"], "properties": Value::Null }),
    )?;
    let list = result
        .get("list")
        .and_then(Value::as_array)
        .ok_or_else(|| CliError::UnexpectedResponse("singleton get missing 'list'".into()))?;
    let Some(item) = list.first() else {
        return Err(CliError::msg(format!(
            "singleton {} returned no value",
            resolve::display_name(canonical)
        )));
    };
    let obj = item
        .as_object()
        .ok_or_else(|| CliError::UnexpectedResponse("singleton result is not an object".into()))?;
    let transformed = transform_object(schema, fields, obj, allow, include_secrets);

    sink.write_all(b"{\"@type\":\"update\",\"object\":\"")?;
    sink.write_all(resolve::display_name(canonical).as_bytes())?;
    sink.write_all(b"\",\"value\":")?;
    serde_json::to_writer(&mut *sink, &Value::Object(transformed))?;
    sink.write_all(b"}\n")?;
    reporter.singleton_done(canonical);
    Ok(())
}

fn write_client_id<W: Write>(sink: &mut W, canonical: &str, id: &str) -> CliResult<()> {
    let prefix = prefix_for(canonical);
    sink.write_all(prefix.as_bytes())?;
    sink.write_all(b"-")?;
    sink.write_all(id.as_bytes())?;
    Ok(())
}

fn prefix_for(canonical: &str) -> String {
    let without_prefix = canonical.strip_prefix("x:").unwrap_or(canonical);
    let base = without_prefix.split('/').next().unwrap_or(without_prefix);
    base.to_ascii_lowercase()
}

fn transform_object(
    schema: &Schema,
    fields: &Fields,
    input: &Map<String, Value>,
    allow: &HashSet<String>,
    include_secrets: bool,
) -> Map<String, Value> {
    let mut out = Map::with_capacity(input.len());
    if let Some(at_type) = input.get("@type") {
        out.insert("@type".into(), at_type.clone());
    }
    for (name, field) in &fields.properties {
        if matches!(field.update, crate::schema::FieldUpdate::ServerSet) {
            continue;
        }
        if !include_secrets && field_is_secret_scalar(&field.typ) {
            continue;
        }
        let Some(raw) = input.get(name) else { continue };
        let transformed = transform_value(schema, &field.typ, raw.clone(), allow, include_secrets);
        let Some(v) = transformed else { continue };
        out.insert(name.clone(), v);
    }
    out
}

fn field_is_secret_scalar(t: &FieldType) -> bool {
    matches!(
        t,
        FieldType::String {
            format: StringFormat::Secret | StringFormat::SecretText,
            ..
        }
    )
}

fn transform_value(
    schema: &Schema,
    t: &FieldType,
    value: Value,
    allow: &HashSet<String>,
    include_secrets: bool,
) -> Option<Value> {
    if value.is_null() {
        return Some(value);
    }
    match t {
        FieldType::ObjectId { object_name, .. } => {
            let (base, _) = split_view(object_name);
            if allow.contains(base) {
                return None;
            }
            let Value::String(id) = value else {
                return Some(Value::Null);
            };
            let prefix = prefix_for(object_name);
            let mut s = String::with_capacity(prefix.len() + id.len() + 2);
            s.push('#');
            s.push_str(&prefix);
            s.push('-');
            s.push_str(&id);
            Some(Value::String(s))
        }
        FieldType::Set { class, .. } => transform_set(schema, class, value, allow),
        FieldType::Map {
            key_class,
            value_class,
            ..
        } => transform_map(
            schema,
            key_class,
            value_class,
            value,
            allow,
            include_secrets,
        ),
        FieldType::Object { object_name, .. } => {
            transform_embedded(schema, object_name, value, allow, include_secrets)
        }
        FieldType::ObjectList { object_name, .. } => {
            transform_object_list(schema, object_name, value, allow, include_secrets)
        }
        _ => Some(value),
    }
}

fn transform_set(
    _schema: &Schema,
    class: &ScalarType,
    value: Value,
    allow: &HashSet<String>,
) -> Option<Value> {
    let Value::Object(map) = value else {
        return Some(value);
    };
    match class {
        ScalarType::ObjectId { object_name } => {
            let (base, _) = split_view(object_name);
            if allow.contains(base) {
                return None;
            }
            let prefix = prefix_for(object_name);
            let mut out = Map::with_capacity(map.len());
            for (id, v) in map {
                let mut key = String::with_capacity(prefix.len() + id.len() + 2);
                key.push('#');
                key.push_str(&prefix);
                key.push('-');
                key.push_str(&id);
                out.insert(key, v);
            }
            Some(Value::Object(out))
        }
        _ => Some(Value::Object(map)),
    }
}

fn transform_map(
    schema: &Schema,
    key_class: &ScalarType,
    value_class: &MapValueType,
    value: Value,
    allow: &HashSet<String>,
    include_secrets: bool,
) -> Option<Value> {
    let Value::Object(map) = value else {
        return Some(value);
    };
    let prefix = match key_class {
        ScalarType::ObjectId { object_name } => {
            let (base, _) = split_view(object_name);
            if allow.contains(base) {
                return None;
            }
            Some(prefix_for(object_name))
        }
        _ => None,
    };
    let mut out = Map::with_capacity(map.len());
    for (k, v) in map {
        let new_key = match &prefix {
            Some(p) => {
                let mut s = String::with_capacity(p.len() + k.len() + 2);
                s.push('#');
                s.push_str(p);
                s.push('-');
                s.push_str(&k);
                s
            }
            None => k,
        };
        let new_value = match value_class {
            MapValueType::Object { object_name } => {
                match transform_embedded(schema, object_name, v, allow, include_secrets) {
                    Some(tv) => tv,
                    None => continue,
                }
            }
            _ => v,
        };
        out.insert(new_key, new_value);
    }
    Some(Value::Object(out))
}

fn transform_embedded(
    schema: &Schema,
    object_name: &str,
    value: Value,
    allow: &HashSet<String>,
    include_secrets: bool,
) -> Option<Value> {
    let Value::Object(map) = value else {
        return Some(value);
    };
    let obj_schema = schema.schemas.get(object_name)?;
    let fields = match obj_schema {
        ObjectSchema::Single { schema_name } => schema.fields.get(schema_name)?,
        ObjectSchema::Multiple { variants } => {
            let at_type = map.get("@type").and_then(Value::as_str)?;
            let v = variants.iter().find(|v| v.name == at_type)?;
            match v.schema_name.as_ref().and_then(|n| schema.fields.get(n)) {
                Some(f) => f,
                None => {
                    let mut out = Map::with_capacity(1);
                    out.insert("@type".into(), Value::String(at_type.to_string()));
                    return Some(Value::Object(out));
                }
            }
        }
    };
    let mut out = Map::with_capacity(map.len());
    if let Some(at_type) = map.get("@type") {
        out.insert("@type".into(), at_type.clone());
    }
    for (name, field) in &fields.properties {
        if matches!(field.update, crate::schema::FieldUpdate::ServerSet) {
            continue;
        }
        if !include_secrets && field_is_secret_scalar(&field.typ) {
            continue;
        }
        let Some(raw) = map.get(name) else { continue };
        if let Some(v) = transform_value(schema, &field.typ, raw.clone(), allow, include_secrets) {
            out.insert(name.clone(), v);
        }
    }
    Some(Value::Object(out))
}

fn transform_object_list(
    schema: &Schema,
    object_name: &str,
    value: Value,
    allow: &HashSet<String>,
    include_secrets: bool,
) -> Option<Value> {
    let Value::Object(map) = value else {
        return Some(value);
    };
    let mut out = Map::with_capacity(map.len());
    for (idx, item) in map {
        if let Some(tv) = transform_embedded(schema, object_name, item, allow, include_secrets) {
            out.insert(idx, tv);
        }
    }
    Some(Value::Object(out))
}

struct Reporter {
    quiet: bool,
}

impl Reporter {
    fn new(quiet: bool) -> Self {
        Reporter { quiet }
    }
    fn plan_header(&self, plan: &Plan) {
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        let destroys = plan.shards.iter().filter(|s| !s.is_singleton).count();
        let _ = writeln!(
            err,
            "snapshot: {} creates, {} singletons",
            destroys,
            plan.singletons.len()
        );
    }
    fn fetch_start(&mut self, canonical: &str) {
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = writeln!(
            err,
            "  fetching {}...",
            resolve::display_name(canonical)
        );
    }
    fn fetch_done(&mut self, count: usize) {
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "    {count} fetched");
    }
    fn shard_done(&mut self, key: &ShardKey, count: usize) {
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        match &key.variant {
            Some(v) => {
                let _ = writeln!(
                    err,
                    "    {} ({}): {count}",
                    resolve::display_name(&key.canonical),
                    v
                );
            }
            None => {
                let _ = writeln!(
                    err,
                    "    {}: {count}",
                    resolve::display_name(&key.canonical)
                );
            }
        }
    }
    fn singleton_start(&mut self, canonical: &str) {
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = writeln!(
            err,
            "  fetching singleton {}...",
            resolve::display_name(canonical)
        );
    }
    fn singleton_done(&mut self, _canonical: &str) {}
    fn done(&mut self) {
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "snapshot complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_for_simple() {
        assert_eq!(prefix_for("x:Domain"), "domain");
        assert_eq!(prefix_for("x:Account"), "account");
        assert_eq!(prefix_for("x:Account/Group"), "account");
        assert_eq!(prefix_for("Mailbox"), "mailbox");
    }

    #[test]
    fn split_view_parsing() {
        assert_eq!(split_view("x:Account/Group"), ("x:Account", Some("Group")));
        assert_eq!(split_view("x:Domain"), ("x:Domain", None));
    }

    #[test]
    fn partition_by_at_type() {
        let list = vec![
            json!({ "id": "a", "@type": "User" }),
            json!({ "id": "b", "@type": "Group" }),
            json!({ "id": "c", "@type": "User" }),
            json!({ "id": "d" }),
        ];
        let mut groups = HashMap::new();
        partition_into(&list, &mut groups);
        assert_eq!(groups[&Some("User".to_string())].len(), 2);
        assert_eq!(groups[&Some("Group".to_string())].len(), 1);
        assert_eq!(groups[&None].len(), 1);
    }

    #[test]
    fn rejects_slash_in_selection() {
        let mut s = Schema::default();
        s.objects.insert(
            "x:Account/User".to_string(),
            ObjectType::View {
                object_name: "x:Account".into(),
            },
        );
        let err = resolve_selection(&s, &["account/user".to_string()]).unwrap_err();
        assert!(format!("{err}").contains("cannot select variants"));
    }

    fn obj_type() -> ObjectType {
        ObjectType::Object {
            description: "".into(),
            permission_prefix: "".into(),
            enterprise: false,
        }
    }

    fn objid_field(object_name: &str, mutable: bool) -> crate::schema::Field {
        crate::schema::Field {
            description: "".into(),
            typ: FieldType::ObjectId {
                object_name: object_name.into(),
                nullable: true,
            },
            update: if mutable {
                FieldUpdate::Mutable
            } else {
                FieldUpdate::Immutable
            },
            enterprise: false,
        }
    }

    fn set_objid_field(object_name: &str, mutable: bool) -> crate::schema::Field {
        crate::schema::Field {
            description: "".into(),
            typ: FieldType::Set {
                class: ScalarType::ObjectId {
                    object_name: object_name.into(),
                },
                min_items: None,
                max_items: None,
            },
            update: if mutable {
                FieldUpdate::Mutable
            } else {
                FieldUpdate::Immutable
            },
            enterprise: false,
        }
    }

    fn single_obj(schema_name: &str) -> ObjectSchema {
        ObjectSchema::Single {
            schema_name: schema_name.into(),
        }
    }

    fn fields_with(properties: Vec<(&str, crate::schema::Field)>) -> Fields {
        let mut props: HashMap<String, crate::schema::Field> = HashMap::new();
        for (k, v) in properties {
            props.insert(k.into(), v);
        }
        Fields {
            properties: props,
            defaults: HashMap::new(),
        }
    }

    fn tenant_role_schema(role_to_tenant_mutable: bool, tenant_to_role_mutable: bool) -> Schema {
        let mut s = Schema::default();
        s.objects.insert("x:Tenant".into(), obj_type());
        s.objects.insert("x:Role".into(), obj_type());
        s.schemas.insert("x:Tenant".into(), single_obj("x:Tenant"));
        s.schemas.insert("x:Role".into(), single_obj("x:Role"));
        s.fields.insert(
            "x:Tenant".into(),
            fields_with(vec![(
                "roles",
                set_objid_field("x:Role", tenant_to_role_mutable),
            )]),
        );
        s.fields.insert(
            "x:Role".into(),
            fields_with(vec![(
                "memberTenantId",
                objid_field("x:Tenant", role_to_tenant_mutable),
            )]),
        );
        s
    }

    #[test]
    fn validate_recommends_allow_unresolved_when_adding_target_creates_cycle() {
        let s = tenant_role_schema(true, true);
        let selection = vec!["x:Tenant".to_string()];
        let err =
            validate_static_refs(&s, &selection, &HashSet::new()).expect_err("expected error");
        let msg = format!("{err}");
        assert!(
            msg.contains("--allow-unresolved Role"),
            "expected --allow-unresolved Role suggestion, got: {msg}"
        );
        assert!(
            msg.contains("cycle"),
            "expected mention of cycle, got: {msg}"
        );
    }

    #[test]
    fn validate_recommends_adding_target_when_no_cycle() {
        let mut s = Schema::default();
        s.objects.insert("x:A".into(), obj_type());
        s.objects.insert("x:B".into(), obj_type());
        s.schemas.insert("x:A".into(), single_obj("x:A"));
        s.schemas.insert("x:B".into(), single_obj("x:B"));
        s.fields.insert(
            "x:A".into(),
            fields_with(vec![("bid", objid_field("x:B", true))]),
        );
        s.fields.insert("x:B".into(), fields_with(vec![]));
        let err = validate_static_refs(&s, &["x:A".to_string()], &HashSet::new())
            .expect_err("expected error");
        let msg = format!("{err}");
        assert!(
            msg.contains("add B to the selection"),
            "expected add B suggestion, got: {msg}"
        );
        assert!(
            !msg.contains("would form a cycle"),
            "expected no cycle text, got: {msg}"
        );
    }

    #[test]
    fn topo_sort_breaks_tenant_role_cycle_via_mutable_edge() {
        let s = tenant_role_schema(true, true);
        let mut shards = vec![
            Shard {
                key: ShardKey {
                    canonical: "x:Tenant".into(),
                    variant: None,
                },
                is_singleton: false,
            },
            Shard {
                key: ShardKey {
                    canonical: "x:Role".into(),
                    variant: None,
                },
                is_singleton: false,
            },
        ];
        let deferred = topologically_sort(&s, &mut shards).expect("must succeed");
        assert_eq!(
            deferred.len(),
            1,
            "expected exactly one deferred edge: {deferred:?}"
        );
        let edge = &deferred[0];
        assert_eq!(edge.from_shard.canonical, "x:Tenant");
        assert_eq!(edge.to_shard.canonical, "x:Role");
        assert_eq!(edge.field, "roles");
        let names: Vec<&str> = shards.iter().map(|s| s.key.canonical.as_str()).collect();
        assert_eq!(names, vec!["x:Tenant", "x:Role"]);
    }

    #[test]
    fn topo_sort_errors_with_only_scc_nodes_when_immutable_cycle() {
        let mut s = tenant_role_schema(false, false);
        s.objects.insert("x:Domain".into(), obj_type());
        s.schemas.insert("x:Domain".into(), single_obj("x:Domain"));
        s.fields.insert(
            "x:Domain".into(),
            fields_with(vec![("memberTenantId", objid_field("x:Tenant", true))]),
        );
        let mut shards = vec![
            Shard {
                key: ShardKey {
                    canonical: "x:Tenant".into(),
                    variant: None,
                },
                is_singleton: false,
            },
            Shard {
                key: ShardKey {
                    canonical: "x:Role".into(),
                    variant: None,
                },
                is_singleton: false,
            },
            Shard {
                key: ShardKey {
                    canonical: "x:Domain".into(),
                    variant: None,
                },
                is_singleton: false,
            },
        ];
        let err = topologically_sort(&s, &mut shards).expect_err("expected cycle error");
        let msg = format!("{err}");
        assert!(
            msg.contains("Tenant") && msg.contains("Role"),
            "expected Tenant and Role in: {msg}"
        );
        assert!(
            !msg.contains("Domain"),
            "Domain should not appear in cycle list: {msg}"
        );
        assert!(
            msg.contains("immutable"),
            "expected mention of immutable field: {msg}"
        );
    }

    #[test]
    fn emit_create_omits_deferred_field_and_emits_followup_update() {
        let s = tenant_role_schema(true, true);
        let plan = Plan {
            shards: vec![
                Shard {
                    key: ShardKey {
                        canonical: "x:Tenant".into(),
                        variant: None,
                    },
                    is_singleton: false,
                },
                Shard {
                    key: ShardKey {
                        canonical: "x:Role".into(),
                        variant: None,
                    },
                    is_singleton: false,
                },
            ],
            singletons: vec![],
            deferred: vec![DeferredEdge {
                from_shard: ShardKey {
                    canonical: "x:Tenant".into(),
                    variant: None,
                },
                to_shard: ShardKey {
                    canonical: "x:Role".into(),
                    variant: None,
                },
                field: "roles".into(),
            }],
        };

        let mut tenant_groups: VariantGroups = HashMap::new();
        let mut tenant = Map::new();
        tenant.insert("id".into(), Value::String("t1".into()));
        tenant.insert(
            "roles".into(),
            json!({"r1": true})
                .as_object()
                .map(|m| Value::Object(m.clone()))
                .unwrap_or(Value::Null),
        );
        tenant_groups.entry(None).or_default().push(tenant);

        let mut role_groups: VariantGroups = HashMap::new();
        let mut role = Map::new();
        role.insert("id".into(), Value::String("r1".into()));
        role.insert("memberTenantId".into(), Value::String("t1".into()));
        role_groups.entry(None).or_default().push(role);

        let mut cache = FetchCache::new();
        cache.by_canonical.insert("x:Tenant".into(), tenant_groups);
        cache.by_canonical.insert("x:Role".into(), role_groups);

        let allow: HashSet<String> = HashSet::new();
        let cfg = crate::app::config::Config {
            url: "http://localhost".into(),
            auth: crate::app::config::AuthMode::Bearer {
                token: "x".into(),
            },
            insecure: false,
            color: false,
        };
        let http = crate::jmap::http::HttpClient::new(&cfg).unwrap();
        let jmap = Jmap::new(&http, "/");
        let cx = Ctx {
            jmap: &jmap,
            schema: &s,
            allow: &allow,
            include_secrets: false,
            limit: 100,
        };

        let mut buf: Vec<u8> = Vec::new();
        {
            let mut sink: &mut dyn Write = &mut buf;
            let mut reporter = Reporter::new(true);
            for shard in plan.iter_non_singletons() {
                let deferred = plan.deferred_for(&shard.key);
                emit_create(
                    &mut sink,
                    &cx,
                    shard,
                    &deferred,
                    &mut cache,
                    &mut reporter,
                )
                .unwrap();
            }
            emit_deferred_updates(&mut sink, &cx, &plan, &mut cache).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 3, "expected 3 ndjson lines, got: {output}");

        let create_tenant: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(create_tenant.get("@type"), Some(&Value::String("create".into())));
        assert_eq!(create_tenant.get("object"), Some(&Value::String("Tenant".into())));
        let value = create_tenant
            .get("value")
            .and_then(Value::as_object)
            .unwrap();
        let tenant_obj = value.values().next().and_then(Value::as_object).unwrap();
        assert!(
            !tenant_obj.contains_key("roles"),
            "expected `roles` to be omitted from create body, got: {tenant_obj:?}"
        );

        let create_role: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(create_role.get("@type"), Some(&Value::String("create".into())));
        assert_eq!(create_role.get("object"), Some(&Value::String("Role".into())));

        let update_tenant: Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(update_tenant.get("@type"), Some(&Value::String("update".into())));
        assert_eq!(update_tenant.get("object"), Some(&Value::String("Tenant".into())));
        let id = update_tenant.get("id").and_then(Value::as_str).unwrap();
        assert!(
            id.starts_with("#tenant-"),
            "expected `#tenant-...` id, got `{id}`"
        );
        let val = update_tenant
            .get("value")
            .and_then(Value::as_object)
            .unwrap();
        assert_eq!(val.len(), 1);
        assert!(val.contains_key("roles"), "expected `roles` in update body");
        let roles = val.get("roles").and_then(Value::as_object).unwrap();
        let key = roles.keys().next().unwrap();
        assert!(
            key.starts_with("#role-"),
            "expected role id to be `#role-...`, got `{key}`"
        );
    }

    #[test]
    fn transform_embedded_preserves_marker_only_variant() {
        use crate::schema::{Field, FieldType, FieldUpdate, Fields, ObjectVariant};
        let mut s = Schema::default();
        s.schemas.insert(
            "x:DkimManagement".into(),
            ObjectSchema::Multiple {
                variants: vec![
                    ObjectVariant {
                        name: "Automatic".into(),
                        label: "".into(),
                        schema_name: Some("x:DkimManagementProperties".into()),
                    },
                    ObjectVariant {
                        name: "Manual".into(),
                        label: "".into(),
                        schema_name: None,
                    },
                ],
            },
        );
        let mut props: HashMap<String, Field> = HashMap::new();
        props.insert(
            "selectorTemplate".into(),
            Field {
                description: "".into(),
                typ: FieldType::String {
                    format: StringFormat::String,
                    min_length: None,
                    max_length: None,
                    nullable: false,
                },
                update: FieldUpdate::Mutable,
                enterprise: false,
            },
        );
        s.fields.insert(
            "x:DkimManagementProperties".into(),
            Fields {
                properties: props,
                defaults: HashMap::new(),
            },
        );

        let manual = json!({"@type": "Manual"});
        let allow: HashSet<String> = HashSet::new();
        let out = transform_embedded(&s, "x:DkimManagement", manual, &allow, false)
            .expect("manual variant must round-trip");
        assert_eq!(out, json!({"@type": "Manual"}));

        let auto = json!({"@type": "Automatic", "selectorTemplate": "v{version}"});
        let out = transform_embedded(&s, "x:DkimManagement", auto, &allow, false)
            .expect("automatic variant must round-trip");
        assert_eq!(
            out,
            json!({"@type": "Automatic", "selectorTemplate": "v{version}"})
        );
    }
}
