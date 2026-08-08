/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::context::Context;
use crate::app::error::{CliError, CliResult};
use crate::cli::ApplyArgs;
use crate::jmap::Jmap;
use crate::jmap::errors::SetError;
use crate::jmap::protocol::{CallResponse, check_response};
use crate::render::Ansi;
use crate::render::set_error;
use crate::schema::resolve;
use crate::schema::{Field, FieldType, Fields, ObjectSchema, ObjectType, Schema, StringFormat};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

pub fn run(ctx: &Context, args: &ApplyArgs) -> CliResult<()> {
    let input = read_input(args)?;
    let raw_ops = parse_ndjson_plan(&input)?;

    let plan = Plan::resolve(&ctx.schema, raw_ops)?;
    validate_plan_references(&ctx.schema, &plan)?;

    let ansi = Ansi::new(ctx.config.color);
    let reporter = Reporter::new(args, ansi);
    reporter.plan_header(&plan);

    if args.dry_run {
        reporter.dry_run_note();
        return Ok(());
    }

    let jmap = Jmap::new(&ctx.client, &ctx.session.api_path);
    let mut state = State {
        created_ids: HashMap::new(),
        touched: HashMap::new(),
        summary: Summary::new(&plan),
    };
    let mut matcher = Matcher::new();
    let mut reconcile_leaks: Vec<(String, usize, Vec<String>)> = Vec::new();

    for op in plan.ops.iter().rev() {
        if let ResolvedOp::Destroy {
            canonical,
            filter,
            index,
        } = op
        {
            let result = execute_destroy(ctx, &jmap, canonical, filter.as_ref(), &reporter);
            match result {
                Ok(count) => {
                    matcher.invalidate(canonical);
                    state.summary.destroyed += count;
                    reporter.op_ok(*index, "destroy", canonical, count);
                }
                Err(e) => {
                    state.summary.failed += 1;
                    reporter.op_err(*index, "destroy", canonical, &e);
                    if !args.continue_on_error {
                        reporter.final_summary(&state.summary);
                        return Err(e);
                    }
                }
            }
        }
    }

    for op in &plan.ops {
        match op {
            ResolvedOp::Destroy { .. } => {}
            ResolvedOp::Upsert {
                canonical,
                match_on,
                scope,
                value,
                index,
            } => {
                match execute_upsert(
                    ctx,
                    &jmap,
                    canonical,
                    match_on.as_ref(),
                    scope.as_ref(),
                    value,
                    &mut state,
                    &mut matcher,
                    *index,
                    &reporter,
                ) {
                    Ok((created, updated)) => {
                        state.summary.created += created;
                        state.summary.updated += updated;
                        reporter.op_ok(*index, "upsert", canonical, created + updated);
                    }
                    Err(e) => {
                        state.summary.failed += 1;
                        reporter.op_err(*index, "upsert", canonical, &e);
                        if !args.continue_on_error {
                            reporter.final_summary(&state.summary);
                            return Err(e);
                        }
                    }
                }
            }
            ResolvedOp::Reconcile {
                canonical,
                match_on,
                scope,
                value,
                index,
            } => {
                match execute_reconcile(
                    ctx,
                    &jmap,
                    canonical,
                    match_on.as_ref(),
                    scope.as_ref(),
                    value,
                    &mut state,
                    &mut matcher,
                    *index,
                    &reporter,
                ) {
                    Ok((created, updated, leaked)) => {
                        state.summary.created += created;
                        state.summary.updated += updated;
                        reporter.op_ok(*index, "reconcile", canonical, created + updated);
                        reconcile_leaks.push((canonical.clone(), *index, leaked));
                    }
                    Err(e) => {
                        state.summary.failed += 1;
                        reporter.op_err(*index, "reconcile", canonical, &e);
                        if !args.continue_on_error {
                            reporter.final_summary(&state.summary);
                            return Err(e);
                        }
                    }
                }
            }
            ResolvedOp::Update {
                canonical,
                id,
                value,
                index,
            } => match execute_update(&jmap, canonical, id, value, &mut state.created_ids) {
                Ok((count, real_id)) => {
                    if let Some(patch) = value.as_object() {
                        matcher.record_updated(canonical, &real_id, patch);
                    }
                    state.touch(canonical, &real_id);
                    state.summary.updated += count;
                    reporter.op_ok(*index, "update", canonical, count);
                }
                Err(e) => {
                    state.summary.failed += 1;
                    reporter.op_err(*index, "update", canonical, &e);
                    if !args.continue_on_error {
                        reporter.final_summary(&state.summary);
                        return Err(e);
                    }
                }
            },
            ResolvedOp::Create {
                canonical,
                value,
                index,
            } => {
                let batch_size = ctx.session.max_objects_in_set.max(1);
                match execute_create(
                    &jmap,
                    canonical,
                    value,
                    batch_size,
                    &mut state.created_ids,
                    *index,
                    &reporter,
                ) {
                    Ok(objects) => {
                        let count = objects.len();
                        for obj in &objects {
                            if let Some(id) = obj.get("id").and_then(Value::as_str) {
                                state.touch(canonical, id);
                            }
                        }
                        matcher.record_created(canonical, objects);
                        state.summary.created += count;
                        reporter.op_ok(*index, "create", canonical, count);
                    }
                    Err(e) => {
                        state.summary.failed += 1;
                        reporter.op_err(*index, "create", canonical, &e);
                        if !args.continue_on_error {
                            reporter.final_summary(&state.summary);
                            return Err(e);
                        }
                    }
                }
            }
        }
    }

    let batch_size = ctx.session.max_objects_in_set.max(1);
    let mut already_destroyed: HashMap<String, HashSet<String>> = HashMap::new();
    for (canonical, index, ids) in reconcile_leaks.iter().rev() {
        let seen = already_destroyed.entry(canonical.clone()).or_default();
        let ids: Vec<String> = ids
            .iter()
            .filter(|id| !state.is_touched(canonical, id) && seen.insert((*id).clone()))
            .cloned()
            .collect();
        if ids.is_empty() {
            continue;
        }
        match destroy_ids(&jmap, canonical, &ids, batch_size, &reporter) {
            Ok(count) => {
                matcher.invalidate(canonical);
                state.summary.destroyed += count;
                reporter.reconcile_cleanup(*index, canonical, count);
            }
            Err(e) => {
                state.summary.failed += 1;
                reporter.op_err(*index, "reconcile", canonical, &e);
                if !args.continue_on_error {
                    reporter.final_summary(&state.summary);
                    return Err(e);
                }
            }
        }
    }

    reporter.final_summary(&state.summary);
    if state.summary.failed > 0 {
        return Err(CliError::msg(format!(
            "apply completed with {} failed operation(s)",
            state.summary.failed
        )));
    }
    Ok(())
}

fn parse_ndjson_plan(input: &str) -> CliResult<Vec<RawOp>> {
    let mut ops = Vec::new();
    for (lineno, line) in input.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let op: RawOp = serde_json::from_str(trimmed).map_err(|e| {
            CliError::msg(format!("invalid plan NDJSON on line {}: {e}", lineno + 1))
        })?;
        ops.push(op);
    }
    Ok(ops)
}

fn read_input(args: &ApplyArgs) -> CliResult<String> {
    match (&args.file, args.stdin) {
        (Some(path), false) => {
            let bytes = std::fs::read(path)?;
            let text = String::from_utf8(bytes)
                .map_err(|e| CliError::msg(format!("{}: invalid UTF-8: {e}", path.display())))?;
            Ok(text)
        }
        (None, true) => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            Ok(buf)
        }
        (Some(_), true) => Err(CliError::msg("--file and --stdin are mutually exclusive")),
        (None, false) => Err(CliError::msg(
            "no plan provided; pass --file <path> or --stdin",
        )),
    }
}

#[derive(Debug, Clone)]
enum MatchOn {
    Wildcard(String),
    Props(Vec<String>),
}

const MATCH_ON_WILDCARD: &str = "*";

impl<'de> Deserialize<'de> for MatchOn {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MatchOnVisitor;

        impl<'de> serde::de::Visitor<'de> for MatchOnVisitor {
            type Value = MatchOn;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a list of property names or the string \"*\"")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<MatchOn, E> {
                Ok(MatchOn::Wildcard(value.to_string()))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<MatchOn, A::Error> {
                let mut props = Vec::new();
                while let Some(prop) = seq.next_element::<String>()? {
                    props.push(prop);
                }
                Ok(MatchOn::Props(props))
            }
        }

        deserializer.deserialize_any(MatchOnVisitor)
    }
}

#[derive(Deserialize, Debug)]
#[serde(tag = "@type", rename_all = "lowercase", deny_unknown_fields)]
enum RawOp {
    Update {
        object: String,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        scope: Option<Value>,
        #[serde(default)]
        value: Value,
    },
    Destroy {
        object: String,
        #[serde(default)]
        scope: Option<Value>,
        #[serde(default)]
        value: Option<Value>,
    },
    Create {
        object: String,
        #[serde(default)]
        scope: Option<Value>,
        #[serde(default)]
        value: Map<String, Value>,
    },
    Upsert {
        object: String,
        #[serde(default, rename = "matchOn")]
        match_on: Option<MatchOn>,
        #[serde(default)]
        scope: Option<Value>,
        #[serde(default)]
        value: Map<String, Value>,
    },
    Reconcile {
        object: String,
        #[serde(default, rename = "matchOn")]
        match_on: Option<MatchOn>,
        #[serde(default)]
        scope: Option<Value>,
        #[serde(default)]
        value: Map<String, Value>,
    },
}

enum ResolvedOp {
    Update {
        canonical: String,
        id: String,
        value: Value,
        index: usize,
    },
    Destroy {
        canonical: String,
        filter: Option<Value>,
        index: usize,
    },
    Create {
        canonical: String,
        value: Map<String, Value>,
        index: usize,
    },
    Upsert {
        canonical: String,
        match_on: Option<MatchOn>,
        scope: Option<Map<String, Value>>,
        value: Map<String, Value>,
        index: usize,
    },
    Reconcile {
        canonical: String,
        match_on: Option<MatchOn>,
        scope: Option<Map<String, Value>>,
        value: Map<String, Value>,
        index: usize,
    },
}

struct Plan {
    ops: Vec<ResolvedOp>,
    destroys: usize,
    updates: usize,
    creates: usize,
    create_objects: usize,
    upserts: usize,
    upsert_objects: usize,
    reconciles: usize,
    reconcile_objects: usize,
}

impl Plan {
    fn resolve(schema: &Schema, raw: Vec<RawOp>) -> CliResult<Self> {
        let mut ops = Vec::with_capacity(raw.len());
        let mut destroys = 0usize;
        let mut updates = 0usize;
        let mut creates = 0usize;
        let mut create_objects = 0usize;
        let mut upserts = 0usize;
        let mut upsert_objects = 0usize;
        let mut reconciles = 0usize;
        let mut reconcile_objects = 0usize;

        for (index, r) in raw.into_iter().enumerate() {
            match r {
                RawOp::Update {
                    object,
                    id,
                    scope,
                    value,
                } => {
                    let canonical = resolve::require_object(schema, &object)?;
                    reject_scope(scope.as_ref(), "update", index)?;
                    let is_singleton = matches!(
                        schema.objects.get(canonical),
                        Some(ObjectType::Singleton { .. })
                    );
                    let id = resolve_update_id(canonical, is_singleton, id.as_deref(), index)?;
                    updates += 1;
                    ops.push(ResolvedOp::Update {
                        canonical: canonical.to_string(),
                        id,
                        value,
                        index,
                    });
                }
                RawOp::Destroy {
                    object,
                    scope,
                    value,
                } => {
                    let canonical = resolve::require_object(schema, &object)?;
                    reject_scope(scope.as_ref(), "destroy", index)?;
                    if matches!(
                        schema.objects.get(canonical),
                        Some(ObjectType::Singleton { .. })
                    ) {
                        return Err(CliError::msg(format!(
                            "cannot destroy singleton `{}` (operation #{})",
                            resolve::display_name(canonical),
                            index + 1,
                        )));
                    }
                    let filter = match value {
                        None | Some(Value::Null) => None,
                        Some(v) => Some(v),
                    };
                    destroys += 1;
                    ops.push(ResolvedOp::Destroy {
                        canonical: canonical.to_string(),
                        filter,
                        index,
                    });
                }
                RawOp::Create {
                    object,
                    scope,
                    value,
                } => {
                    let canonical = resolve::require_object(schema, &object)?;
                    reject_scope(scope.as_ref(), "create", index)?;
                    if value.is_empty() {
                        return Err(CliError::msg(format!(
                            "create operation #{} has an empty `value` map (no objects to create)",
                            index + 1,
                        )));
                    }

                    let mut normalised = Map::new();
                    for (k, v) in value {
                        let key = k.strip_prefix('#').map(String::from).unwrap_or(k);
                        if normalised.contains_key(&key) {
                            return Err(CliError::msg(format!(
                                "create operation #{} has duplicate id `{}`",
                                index + 1,
                                key
                            )));
                        }
                        normalised.insert(key, v);
                    }
                    creates += 1;
                    create_objects += normalised.len();
                    ops.push(ResolvedOp::Create {
                        canonical: canonical.to_string(),
                        value: normalised,
                        index,
                    });
                }
                RawOp::Upsert {
                    object,
                    match_on,
                    scope,
                    value,
                } => {
                    let canonical = resolve::require_object(schema, &object)?;
                    if matches!(
                        schema.objects.get(canonical),
                        Some(ObjectType::Singleton { .. })
                    ) {
                        return Err(CliError::msg(format!(
                            "cannot upsert singleton `{}` (operation #{}); use update instead",
                            resolve::display_name(canonical),
                            index + 1,
                        )));
                    }
                    if value.is_empty() {
                        return Err(CliError::msg(format!(
                            "upsert operation #{} has an empty `value` map (no objects to upsert)",
                            index + 1,
                        )));
                    }
                    validate_match_on(match_on.as_ref(), "upsert", index)?;
                    let scope =
                        resolve_scope_field(schema, canonical, scope, &value, "upsert", index)?;

                    let mut normalised = Map::new();
                    for (k, v) in value {
                        let key = k.strip_prefix('#').map(String::from).unwrap_or(k);
                        if normalised.contains_key(&key) {
                            return Err(CliError::msg(format!(
                                "upsert operation #{} has duplicate id `{}`",
                                index + 1,
                                key
                            )));
                        }
                        normalised.insert(key, v);
                    }
                    upserts += 1;
                    upsert_objects += normalised.len();
                    ops.push(ResolvedOp::Upsert {
                        canonical: canonical.to_string(),
                        match_on,
                        scope,
                        value: normalised,
                        index,
                    });
                }
                RawOp::Reconcile {
                    object,
                    match_on,
                    scope,
                    value,
                } => {
                    let canonical = resolve::require_object(schema, &object)?;
                    if matches!(
                        schema.objects.get(canonical),
                        Some(ObjectType::Singleton { .. })
                    ) {
                        return Err(CliError::msg(format!(
                            "cannot reconcile singleton `{}` (operation #{}); use update instead",
                            resolve::display_name(canonical),
                            index + 1,
                        )));
                    }
                    validate_match_on(match_on.as_ref(), "reconcile", index)?;
                    if match_on.is_none()
                        && matches!(resolve_match_key(schema, canonical, None), MatchKey::Value)
                    {
                        return Err(CliError::msg(format!(
                            "reconcile operation #{} on `{}` has no match key; add a `matchOn` \
                             (or `\"matchOn\": \"*\"` to match by value, which deletes and \
                             recreates any drifted object)",
                            index + 1,
                            resolve::display_name(canonical),
                        )));
                    }
                    let scope =
                        resolve_scope_field(schema, canonical, scope, &value, "reconcile", index)?;

                    let mut normalised = Map::new();
                    for (k, v) in value {
                        let key = k.strip_prefix('#').map(String::from).unwrap_or(k);
                        if normalised.contains_key(&key) {
                            return Err(CliError::msg(format!(
                                "reconcile operation #{} has duplicate id `{}`",
                                index + 1,
                                key
                            )));
                        }
                        normalised.insert(key, v);
                    }
                    reconciles += 1;
                    reconcile_objects += normalised.len();
                    ops.push(ResolvedOp::Reconcile {
                        canonical: canonical.to_string(),
                        match_on,
                        scope,
                        value: normalised,
                        index,
                    });
                }
            }
        }

        Ok(Plan {
            ops,
            destroys,
            updates,
            creates,
            create_objects,
            upserts,
            upsert_objects,
            reconciles,
            reconcile_objects,
        })
    }
}

fn resolve_update_id(
    canonical: &str,
    is_singleton: bool,
    id: Option<&str>,
    index: usize,
) -> CliResult<String> {
    if is_singleton {
        match id {
            None | Some("singleton") => Ok("singleton".to_string()),
            Some(_) => Err(CliError::BadSingletonId),
        }
    } else {
        match id {
            Some(v) if !v.is_empty() => Ok(v.to_string()),
            _ => Err(CliError::msg(format!(
                "update operation #{} on `{}` is missing the top-level `id` field \
                 (required for non-singletons; `id` is a sibling of `value`, \
                 not a key inside it)",
                index + 1,
                resolve::display_name(canonical),
            ))),
        }
    }
}

struct State {
    created_ids: HashMap<String, String>,
    touched: HashMap<String, HashSet<String>>,
    summary: Summary,
}

impl State {
    fn touch(&mut self, canonical: &str, id: &str) {
        let ids = self.touched.entry(canonical.to_string()).or_default();
        if !ids.contains(id) {
            ids.insert(id.to_string());
        }
    }

    fn is_touched(&self, canonical: &str, id: &str) -> bool {
        self.touched
            .get(canonical)
            .is_some_and(|ids| ids.contains(id))
    }
}

struct Summary {
    planned_destroys: usize,
    planned_updates: usize,
    planned_creates: usize,
    planned_create_objects: usize,
    planned_upserts: usize,
    planned_upsert_objects: usize,
    planned_reconciles: usize,
    planned_reconcile_objects: usize,
    destroyed: usize,
    updated: usize,
    created: usize,
    failed: usize,
}

impl Summary {
    fn new(plan: &Plan) -> Self {
        Summary {
            planned_destroys: plan.destroys,
            planned_updates: plan.updates,
            planned_creates: plan.creates,
            planned_create_objects: plan.create_objects,
            planned_upserts: plan.upserts,
            planned_upsert_objects: plan.upsert_objects,
            planned_reconciles: plan.reconciles,
            planned_reconcile_objects: plan.reconcile_objects,
            destroyed: 0,
            updated: 0,
            created: 0,
            failed: 0,
        }
    }
}

fn collect_refs(value: &Value, out: &mut HashSet<String>) {
    match value {
        Value::String(s) => {
            if let Some(id) = s.strip_prefix('#')
                && !id.is_empty()
            {
                out.insert(id.to_string());
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_refs(v, out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                if let Some(id) = k.strip_prefix('#')
                    && !id.is_empty()
                {
                    out.insert(id.to_string());
                }
                collect_refs(v, out);
            }
        }
        _ => {}
    }
}

fn reference_labels<'a>(
    refs: &HashSet<String>,
    state_ids: &'a HashMap<String, String>,
) -> set_error::ClientIds<'a> {
    let mut labels = set_error::ClientIds::new();
    let mut ambiguous: Vec<&str> = Vec::new();
    for r in refs {
        let Some((client_id, server_id)) = state_ids.get_key_value(r.as_str()) else {
            continue;
        };
        if labels
            .insert(server_id.as_str(), client_id.as_str())
            .is_some()
        {
            ambiguous.push(server_id.as_str());
        }
    }
    for server_id in ambiguous {
        labels.remove(server_id);
    }
    labels
}

fn request_created_ids(
    refs: &HashSet<String>,
    state_ids: &HashMap<String, String>,
    created_in_request: &HashSet<String>,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for r in refs {
        if created_in_request.contains(r) {
            continue;
        }
        if let Some(server_id) = state_ids.get(r) {
            out.insert(r.clone(), server_id.clone());
        }
    }
    out
}

fn execute_destroy(
    ctx: &Context,
    jmap: &Jmap,
    canonical: &str,
    filter: Option<&Value>,
    reporter: &Reporter,
) -> CliResult<usize> {
    let ids = fetch_all_ids(ctx, jmap, canonical, filter)?;
    let batch_size = ctx.session.max_objects_in_set.max(1);
    destroy_ids(jmap, canonical, &ids, batch_size, reporter)
}

fn destroy_ids(
    jmap: &Jmap,
    canonical: &str,
    ids: &[String],
    batch_size: usize,
    reporter: &Reporter,
) -> CliResult<usize> {
    if ids.is_empty() {
        return Ok(0);
    }

    let method = format!("{canonical}/set");
    let mut destroyed = 0usize;
    let batches = ids.len().div_ceil(batch_size);

    for (n, chunk) in ids.chunks(batch_size).enumerate() {
        if reporter.progress {
            reporter.batch_note(&format!(
                "destroying {} batch {}/{} ({} ids)",
                resolve::display_name(canonical),
                n + 1,
                batches,
                chunk.len()
            ));
        }
        let args = json!({
            "destroy": chunk.iter().map(|s| Value::String(s.clone())).collect::<Vec<_>>()
        });
        let result = jmap.call(&method, args)?;
        let succeeded = result
            .get("destroyed")
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0);
        destroyed += succeeded;

        if let Some(fails) = result.get("notDestroyed").and_then(Value::as_object)
            && !fails.is_empty()
        {
            let (first_id, first_err) = fails
                .iter()
                .next()
                .map(|(k, v)| (k.clone(), v.clone()))
                .unwrap_or_default();
            let set_err: SetError = serde_json::from_value(first_err).unwrap_or_default();
            return Err(CliError::msg(format!(
                "{}: destroy failed for id {}: {}",
                resolve::display_name(canonical),
                first_id,
                set_error::render(&set_err, Ansi::new(false)).replace('\n', " | ")
            )));
        }
    }
    Ok(destroyed)
}

fn fetch_all_ids(
    ctx: &Context,
    jmap: &Jmap,
    canonical: &str,
    filter: Option<&Value>,
) -> CliResult<Vec<String>> {
    let method = format!("{canonical}/query");
    let limit = ctx.session.max_objects_in_get.max(1);
    let mut all = Vec::new();
    let mut anchor: Option<String> = None;

    loop {
        let mut args = Map::new();
        if let Some(f) = filter {
            args.insert("filter".to_string(), f.clone());
        }
        args.insert("limit".to_string(), Value::from(limit));
        if let Some(a) = &anchor {
            args.insert("anchor".to_string(), Value::String(a.clone()));
            args.insert("anchorOffset".to_string(), Value::from(1));
        } else {
            args.insert("calculateTotal".to_string(), Value::Bool(true));
        }
        let result = jmap.call(&method, Value::Object(args))?;
        let ids: Vec<String> = result
            .get("ids")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            break;
        }
        let last = ids.last().cloned();
        let count = ids.len();
        all.extend(ids);
        if count < limit {
            break;
        }
        anchor = last;
    }
    Ok(all)
}

enum MatchKey {
    Props(Vec<String>),
    Value,
}

fn resolve_scope_field(
    schema: &Schema,
    canonical: &str,
    scope: Option<Value>,
    value: &Map<String, Value>,
    op: &str,
    index: usize,
) -> CliResult<Option<Map<String, Value>>> {
    let scope = match scope {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Object(s)) if s.is_empty() => return Ok(None),
        Some(Value::Object(s)) => s,
        Some(_) => {
            return Err(CliError::msg(format!(
                "{op} operation #{} has a `scope` that is not a JSON object",
                index + 1,
            )));
        }
    };

    let known = scope_properties(schema, canonical);
    for (prop, wanted) in &scope {
        if wanted.is_null() {
            return Err(CliError::msg(format!(
                "{op} operation #{} has a null `scope` value for `{}`; a null matches every \
                 object that does not set the property, which would widen the scope rather \
                 than narrow it",
                index + 1,
                prop,
            )));
        }
        if prop == "@type" {
            let declared = variant_names(schema, canonical);
            if declared.is_empty() {
                return Err(CliError::msg(format!(
                    "{op} operation #{} has a `scope` on `@type`, but {} is not a \
                     multi-variant type",
                    index + 1,
                    resolve::display_name(canonical),
                )));
            }
            if !wanted.as_str().is_some_and(|w| declared.contains(&w)) {
                let mut names: Vec<&str> = declared.into_iter().collect();
                names.sort_unstable();
                return Err(CliError::msg(format!(
                    "{op} operation #{} has a `scope` on `@type` = {}, which is not a variant \
                     of {}; expected one of {}",
                    index + 1,
                    wanted,
                    resolve::display_name(canonical),
                    names.join(", "),
                )));
            }
            continue;
        }
        if scope_prop_is_server_set(schema, canonical, prop) {
            return Err(CliError::msg(format!(
                "{op} operation #{} has a `scope` on `{}`, which the server derives; an entry \
                 cannot be created into that scope, so it would be recreated on every apply",
                index + 1,
                prop,
            )));
        }
        if !known.is_empty() && !known.contains(prop.as_str()) {
            return Err(CliError::msg(format!(
                "{op} operation #{} has a `scope` on `{}`, which is not a property of {}; \
                 the scope is matched client-side against property values, so operator forms \
                 such as `{}Contains` are not supported",
                index + 1,
                prop,
                resolve::display_name(canonical),
                prop,
            )));
        }
    }

    for (client_id, body_val) in value {
        let Some(body) = body_val.as_object() else {
            continue;
        };
        for (prop, wanted) in &scope {
            match body.get(prop) {
                Some(actual)
                    if actual != wanted
                        && client_ref(actual).is_none() == client_ref(wanted).is_none() =>
                {
                    return Err(CliError::msg(format!(
                        "{op} operation #{} declares `{}` with `{}` = {}, which is outside its \
                         own `scope` ({} = {}); an out-of-scope entry can never match an \
                         existing object, so it would be created again on every apply. Move it \
                         to its own operation.",
                        index + 1,
                        client_id,
                        prop,
                        actual,
                        prop,
                        wanted,
                    )));
                }
                _ => {}
            }
        }
    }

    Ok(Some(scope))
}

fn scope_prop_is_server_set(schema: &Schema, canonical: &str, prop: &str) -> bool {
    let is_server_set = |fields: Option<&Fields>| {
        fields
            .and_then(|f| f.properties.get(prop))
            .is_some_and(|f| matches!(f.update, crate::schema::FieldUpdate::ServerSet))
    };
    match schema.schemas.get(canonical) {
        Some(ObjectSchema::Single { schema_name }) => is_server_set(schema.fields.get(schema_name)),
        Some(ObjectSchema::Multiple { variants }) => variants
            .iter()
            .any(|v| is_server_set(v.schema_name.as_ref().and_then(|sn| schema.fields.get(sn)))),
        None => false,
    }
}

fn variant_names<'a>(schema: &'a Schema, canonical: &str) -> HashSet<&'a str> {
    match schema.schemas.get(canonical) {
        Some(ObjectSchema::Multiple { variants }) => {
            variants.iter().map(|v| v.name.as_str()).collect()
        }
        _ => HashSet::new(),
    }
}

fn scope_properties<'a>(schema: &'a Schema, canonical: &str) -> HashSet<&'a str> {
    let mut names = HashSet::new();
    let mut add = |fields: Option<&'a Fields>| {
        if let Some(f) = fields {
            names.extend(f.properties.keys().map(String::as_str));
        }
    };
    match schema.schemas.get(canonical) {
        Some(ObjectSchema::Single { schema_name }) => add(schema.fields.get(schema_name)),
        Some(ObjectSchema::Multiple { variants }) => {
            for v in variants {
                add(v.schema_name.as_ref().and_then(|sn| schema.fields.get(sn)));
            }
        }
        None => {}
    }
    names
}

fn reject_scope(scope: Option<&Value>, op: &str, index: usize) -> CliResult<()> {
    match scope {
        None | Some(Value::Null) => Ok(()),
        Some(_) => Err(CliError::msg(format!(
            "{op} operation #{} has a `scope`; only upsert and reconcile match against \
             existing objects",
            index + 1,
        ))),
    }
}

fn validate_match_on(match_on: Option<&MatchOn>, op: &str, index: usize) -> CliResult<()> {
    match match_on {
        Some(MatchOn::Props(keys)) if keys.is_empty() => Err(CliError::msg(format!(
            "{op} operation #{} has an empty `matchOn` list",
            index + 1,
        ))),
        Some(MatchOn::Wildcard(w)) if w != MATCH_ON_WILDCARD => Err(CliError::msg(format!(
            "{op} operation #{} has an invalid `matchOn` value `{w}`; expected a list of \
             property names or `\"*\"`",
            index + 1,
        ))),
        _ => Ok(()),
    }
}

fn resolve_match_key(schema: &Schema, canonical: &str, match_on: Option<&MatchOn>) -> MatchKey {
    match match_on {
        Some(MatchOn::Props(keys)) => return MatchKey::Props(keys.clone()),
        Some(MatchOn::Wildcard(_)) => return MatchKey::Value,
        None => {}
    }
    if let Some(label) = schema
        .lists
        .get(canonical)
        .and_then(|l| l.label_property.as_ref())
    {
        return MatchKey::Props(vec![label.clone()]);
    }
    MatchKey::Value
}

fn is_multi_variant(schema: &Schema, canonical: &str) -> bool {
    matches!(
        schema.schemas.get(canonical),
        Some(ObjectSchema::Multiple { .. })
    )
}

fn fields_for<'a>(
    schema: &'a Schema,
    canonical: &str,
    at_type: Option<&str>,
) -> Option<&'a Fields> {
    match schema.schemas.get(canonical)? {
        ObjectSchema::Single { schema_name } => schema.fields.get(schema_name),
        ObjectSchema::Multiple { variants } => {
            let at = at_type?;
            let variant = variants.iter().find(|v| v.name == at)?;
            variant
                .schema_name
                .as_ref()
                .and_then(|sn| schema.fields.get(sn))
        }
    }
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

fn is_comparable_scalar(field: &Field) -> bool {
    if matches!(field.update, crate::schema::FieldUpdate::ServerSet) {
        return false;
    }
    if field_is_secret_scalar(&field.typ) {
        return false;
    }
    matches!(
        field.typ,
        FieldType::String { .. }
            | FieldType::Number { .. }
            | FieldType::Boolean
            | FieldType::Enum { .. }
            | FieldType::UtcDateTime { .. }
    )
}

struct Matcher {
    objects: HashMap<String, Vec<Map<String, Value>>>,
    warned_value_match: HashSet<String>,
}

impl Matcher {
    fn new() -> Self {
        Matcher {
            objects: HashMap::new(),
            warned_value_match: HashSet::new(),
        }
    }

    fn ensure(&mut self, ctx: &Context, jmap: &Jmap, canonical: &str) -> CliResult<()> {
        if self.objects.contains_key(canonical) {
            return Ok(());
        }
        let objs = fetch_all_objects(ctx, jmap, canonical)?;
        self.objects.insert(canonical.to_string(), objs);
        Ok(())
    }

    fn objects_for(&self, canonical: &str) -> &[Map<String, Value>] {
        self.objects
            .get(canonical)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn record_created(&mut self, canonical: &str, objects: Vec<Map<String, Value>>) {
        if let Some(cached) = self.objects.get_mut(canonical) {
            cached.extend(objects);
        }
    }

    fn record_updated(&mut self, canonical: &str, id: &str, patch: &Map<String, Value>) {
        let Some(cached) = self.objects.get_mut(canonical) else {
            return;
        };
        let Some(obj) = cached
            .iter_mut()
            .find(|o| o.get("id").and_then(Value::as_str) == Some(id))
        else {
            return;
        };
        for (prop, value) in patch {
            if prop.contains('/') {
                continue;
            }
            if value.is_null() {
                obj.remove(prop);
            } else {
                obj.insert(prop.clone(), value.clone());
            }
        }
    }

    fn invalidate(&mut self, canonical: &str) {
        self.objects.remove(canonical);
    }
}

fn fetch_all_objects(
    ctx: &Context,
    jmap: &Jmap,
    canonical: &str,
) -> CliResult<Vec<Map<String, Value>>> {
    let query_method = format!("{canonical}/query");
    let get_method = format!("{canonical}/get");
    let limit = ctx.session.max_objects_in_get.max(1);
    let mut anchor: Option<String> = None;
    let mut all = Vec::new();

    loop {
        let mut q_args = Map::new();
        q_args.insert("filter".into(), json!({}));
        q_args.insert("limit".into(), Value::from(limit));
        if anchor.is_none() {
            q_args.insert("calculateTotal".into(), Value::Bool(true));
        }
        if let Some(a) = &anchor {
            q_args.insert("anchor".into(), Value::String(a.clone()));
            q_args.insert("anchorOffset".into(), Value::from(1));
        }
        let get_args = json!({
            "#ids": { "resultOf": "q", "name": query_method, "path": "/ids" },
            "properties": Value::Null,
        });

        let resp = jmap.call_with(
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
        let q_result = check_response(q_resp, &query_method)?;
        let g_result = check_response(g_resp, &get_method)?;

        let Some(ids) = q_result.get("ids").and_then(Value::as_array) else {
            break;
        };
        if ids.is_empty() {
            break;
        }
        let last_id = ids
            .last()
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| {
                CliError::UnexpectedResponse(format!("{query_method} returned a non-string id"))
            })?;
        let returned = ids.len();

        if let Some(list) = g_result.get("list").and_then(Value::as_array) {
            for item in list {
                if let Some(obj) = item.as_object() {
                    all.push(obj.clone());
                }
            }
        }

        if returned < limit {
            break;
        }
        anchor = Some(last_id);
    }
    Ok(all)
}

fn wanted_match_values<'a>(
    schema: &Schema,
    canonical: &str,
    body: &Map<String, Value>,
    props: &'a [String],
    created_ids: &HashMap<String, String>,
) -> CliResult<Vec<(&'a str, Value)>> {
    let at_type = body.get("@type").and_then(Value::as_str);
    let fields = fields_for(schema, canonical, at_type);
    let mut wanted = Vec::with_capacity(props.len());
    for p in props {
        let Some(raw) = body.get(p) else {
            return Err(CliError::msg(format!(
                "{}: match property `{}` is missing from the object body",
                resolve::display_name(canonical),
                p,
            )));
        };
        let resolved = resolve_match_value(canonical, fields, p, raw, created_ids)?;
        wanted.push((p.as_str(), resolved));
    }
    Ok(wanted)
}

fn match_signature(
    schema: &Schema,
    canonical: &str,
    body: &Map<String, Value>,
    props: &[String],
    created_ids: &HashMap<String, String>,
) -> CliResult<String> {
    let wanted = wanted_match_values(schema, canonical, body, props, created_ids)?;
    let mut parts = Vec::with_capacity(wanted.len() + 1);
    parts.push(match body.get("@type") {
        Some(t) => t.clone(),
        None => Value::Null,
    });
    for (_, v) in wanted {
        parts.push(v);
    }
    Ok(Value::Array(parts).to_string())
}

fn reject_duplicate_match_keys(
    schema: &Schema,
    canonical: &str,
    entries: &[(&String, Cow<'_, Map<String, Value>>)],
    key: &MatchKey,
    created_ids: &HashMap<String, String>,
    op_index: usize,
) -> CliResult<()> {
    let mut seen: HashMap<String, &str> = HashMap::new();
    let props = match key {
        MatchKey::Props(props) => props.clone(),
        MatchKey::Value => {
            let mut seen: HashMap<String, &str> = HashMap::new();
            for (client_id, body) in entries {
                let body = body.as_ref();
                let at_type = body.get("@type").and_then(Value::as_str);
                let Some(fields) = fields_for(schema, canonical, at_type) else {
                    continue;
                };
                let mut parts: Vec<Value> = vec![match body.get("@type") {
                    Some(t) => t.clone(),
                    None => Value::Null,
                }];
                let mut names: Vec<&str> = fields
                    .properties
                    .iter()
                    .filter(|(_, f)| is_comparable_scalar(f))
                    .map(|(n, _)| n.as_str())
                    .collect();
                names.sort_unstable();
                for n in names {
                    parts.push(body.get(n).cloned().unwrap_or(Value::Null));
                }
                let signature = Value::Array(parts).to_string();
                if let Some(first) = seen.insert(signature, client_id.as_str()) {
                    return Err(CliError::msg(format!(
                        "{}: operation #{} has two entries (`{}`, `{}`) with identical values; \
                         value matching cannot tell them apart, so the second would be created \
                         again on every apply",
                        resolve::display_name(canonical),
                        op_index + 1,
                        first,
                        client_id,
                    )));
                }
            }
            return Ok(());
        }
    };
    let props = &props;
    for (client_id, body) in entries {
        let Ok(signature) = match_signature(schema, canonical, body.as_ref(), props, created_ids)
        else {
            continue;
        };
        if let Some(first) = seen.insert(signature, client_id.as_str()) {
            return Err(CliError::msg(format!(
                "{}: operation #{} has two entries (`{}`, `{}`) with the same match key on {}; \
                 `matchOn` must uniquely identify each entry",
                resolve::display_name(canonical),
                op_index + 1,
                first,
                client_id,
                props.join(", "),
            )));
        }
    }
    Ok(())
}

fn find_match(
    matcher: &Matcher,
    schema: &Schema,
    canonical: &str,
    body: &Map<String, Value>,
    key: &MatchKey,
    scope: Option<&[(String, Value)]>,
    created_ids: &HashMap<String, String>,
) -> CliResult<Option<String>> {
    let multi = is_multi_variant(schema, canonical);
    let at_type = body.get("@type").and_then(Value::as_str);
    if multi && at_type.is_none() {
        return Err(CliError::msg(format!(
            "{}: upsert entry is missing `@type` (required to match a multi-variant object)",
            resolve::display_name(canonical),
        )));
    }

    let candidates = matcher.objects_for(canonical).iter().filter(|o| {
        if multi && o.get("@type").and_then(Value::as_str) != at_type {
            return false;
        }
        in_scope(o, scope)
    });

    match key {
        MatchKey::Props(props) => {
            let wanted = wanted_match_values(schema, canonical, body, props, created_ids)?;
            let mut matched: Option<String> = None;
            let mut count = 0usize;
            for cand in candidates {
                if wanted
                    .iter()
                    .all(|(p, r)| values_match(cand.get(*p), Some(r)))
                {
                    count += 1;
                    if matched.is_none() {
                        matched = cand.get("id").and_then(Value::as_str).map(String::from);
                    }
                }
            }
            if count > 1 {
                return Err(CliError::msg(format!(
                    "{}: ambiguous upsert; {} existing objects match on {}",
                    resolve::display_name(canonical),
                    count,
                    props.join(", "),
                )));
            }
            Ok(matched)
        }
        MatchKey::Value => {
            let fields = fields_for(schema, canonical, at_type).ok_or_else(|| {
                CliError::msg(format!(
                    "{}: cannot determine a match key (no label property and no schema fields); \
                     add a `matchOn` to the plan",
                    resolve::display_name(canonical),
                ))
            })?;
            let props: Vec<&str> = fields
                .properties
                .iter()
                .filter(|(_, f)| is_comparable_scalar(f))
                .map(|(n, _)| n.as_str())
                .collect();
            if props.is_empty() {
                return Err(CliError::msg(format!(
                    "{}: no comparable properties to match on; add a `matchOn` to the plan",
                    resolve::display_name(canonical),
                )));
            }
            let mut matched: Option<String> = None;
            let mut count = 0usize;
            for cand in candidates {
                if props
                    .iter()
                    .all(|p| values_match(cand.get(*p), body.get(*p)))
                {
                    count += 1;
                    if matched.is_none() {
                        matched = cand.get("id").and_then(Value::as_str).map(String::from);
                    }
                }
            }
            if count > 1 {
                return Err(CliError::msg(format!(
                    "{}: ambiguous match; {} existing objects have the same values; \
                     add a `matchOn` naming the properties that identify the object",
                    resolve::display_name(canonical),
                    count,
                )));
            }
            Ok(matched)
        }
    }
}

fn validate_plan_references(schema: &Schema, plan: &Plan) -> CliResult<()> {
    let mut declared: HashMap<String, String> = HashMap::new();
    for op in &plan.ops {
        if let ResolvedOp::Upsert {
            canonical,
            scope: Some(scope),
            ..
        }
        | ResolvedOp::Reconcile {
            canonical,
            scope: Some(scope),
            ..
        } = op
        {
            resolve_scope(schema, canonical, scope, &declared)?;
        }

        let value = match op {
            ResolvedOp::Create { value, .. }
            | ResolvedOp::Upsert { value, .. }
            | ResolvedOp::Reconcile { value, .. } => value,
            _ => continue,
        };
        for k in value.keys() {
            declared.insert(k.clone(), k.clone());
        }

        let (canonical, match_on) = match op {
            ResolvedOp::Upsert {
                canonical,
                match_on,
                ..
            }
            | ResolvedOp::Reconcile {
                canonical,
                match_on,
                ..
            } => (canonical, match_on),
            _ => continue,
        };
        let key = resolve_match_key(schema, canonical, match_on.as_ref());
        let MatchKey::Props(props) = &key else {
            continue;
        };
        for body_val in value.values() {
            let Some(body) = body_val.as_object() else {
                continue;
            };
            let at_type = body.get("@type").and_then(Value::as_str);
            let fields = fields_for(schema, canonical, at_type);
            for p in props {
                if let Some(raw) = body.get(p) {
                    resolve_match_value(canonical, fields, p, raw, &declared)?;
                }
            }
        }
    }
    Ok(())
}

fn resolve_match_value(
    canonical: &str,
    fields: Option<&Fields>,
    prop: &str,
    raw: &Value,
    created_ids: &HashMap<String, String>,
) -> CliResult<Value> {
    let is_ref = fields
        .and_then(|f| f.properties.get(prop))
        .is_some_and(|f| matches!(f.typ, FieldType::ObjectId { .. }));
    if !is_ref {
        return Ok(raw.clone());
    }
    let Some(client_id) = raw
        .as_str()
        .and_then(|s| s.strip_prefix('#'))
        .filter(|s| !s.is_empty())
    else {
        return Ok(raw.clone());
    };
    match created_ids.get(client_id) {
        Some(server_id) => Ok(Value::String(server_id.clone())),
        None => Err(CliError::msg(format!(
            "{}: match property `{}` references unresolved id `#{}` \
             (no create, upsert, or reconcile operation in this plan produced it)",
            resolve::display_name(canonical),
            prop,
            client_id,
        ))),
    }
}

struct UpsertOutcome {
    created: usize,
    updated: usize,
    matched_ids: HashSet<String>,
    scope: Option<Vec<(String, Value)>>,
}

#[allow(clippy::too_many_arguments)]
fn upsert_core(
    ctx: &Context,
    jmap: &Jmap,
    canonical: &str,
    match_on: Option<&MatchOn>,
    scope: Option<&Map<String, Value>>,
    value: &Map<String, Value>,
    state: &mut State,
    matcher: &mut Matcher,
    op_index: usize,
    reporter: &Reporter,
) -> CliResult<UpsertOutcome> {
    matcher.ensure(ctx, jmap, canonical)?;
    let scope = match scope {
        Some(s) => Some(resolve_scope(
            &ctx.schema,
            canonical,
            s,
            &state.created_ids,
        )?),
        None => None,
    };
    let key = resolve_match_key(&ctx.schema, canonical, match_on);
    if matches!(key, MatchKey::Value)
        && match_on.is_none()
        && matcher.warned_value_match.insert(canonical.to_string())
    {
        reporter.value_match_warning(canonical);
    }
    let mut entries: Vec<(&String, Cow<'_, Map<String, Value>>)> = Vec::with_capacity(value.len());
    for (client_id, body_val) in value {
        let body = body_val.as_object().ok_or_else(|| {
            CliError::msg(format!(
                "{}: upsert entry `{}` is not a JSON object",
                resolve::display_name(canonical),
                client_id,
            ))
        })?;
        entries.push((client_id, scoped_body(body, scope.as_deref())));
    }

    reject_duplicate_match_keys(
        &ctx.schema,
        canonical,
        &entries,
        &key,
        &state.created_ids,
        op_index,
    )?;

    let mut to_create: Map<String, Value> = Map::new();
    let mut to_update: Vec<(String, Value)> = Vec::new();
    let mut matched_ids: HashSet<String> = HashSet::new();
    for (client_id, body) in &entries {
        let body = body.as_ref();
        match find_match(
            matcher,
            &ctx.schema,
            canonical,
            body,
            &key,
            scope.as_deref(),
            &state.created_ids,
        )? {
            Some(server_id) => {
                matched_ids.insert(server_id.clone());
                state
                    .created_ids
                    .insert((*client_id).clone(), server_id.clone());
                let at_type = body.get("@type").and_then(Value::as_str);
                let mut patch = body.clone();
                patch.remove("@type");
                if let Some(fields) = fields_for(&ctx.schema, canonical, at_type) {
                    patch.retain(|k, _| {
                        fields
                            .properties
                            .get(k)
                            .map(|f| {
                                !matches!(
                                    f.update,
                                    crate::schema::FieldUpdate::Immutable
                                        | crate::schema::FieldUpdate::ServerSet
                                )
                            })
                            .unwrap_or(true)
                    });
                }
                if !patch.is_empty() {
                    to_update.push((server_id, Value::Object(patch)));
                }
            }
            None => {
                let at_type = body.get("@type").and_then(Value::as_str);
                let mut create_body = body.clone();
                if let Some(fields) = fields_for(&ctx.schema, canonical, at_type) {
                    create_body.retain(|k, _| {
                        fields
                            .properties
                            .get(k)
                            .map(|f| !matches!(f.update, crate::schema::FieldUpdate::ServerSet))
                            .unwrap_or(true)
                    });
                }
                to_create.insert((*client_id).clone(), Value::Object(create_body));
            }
        }
    }

    let mut created = 0usize;
    if !to_create.is_empty() {
        let batch_size = ctx.session.max_objects_in_set.max(1);
        let objects = execute_create(
            jmap,
            canonical,
            &to_create,
            batch_size,
            &mut state.created_ids,
            op_index,
            reporter,
        )?;
        created = objects.len();
        for obj in &objects {
            if let Some(id) = obj.get("id").and_then(Value::as_str) {
                matched_ids.insert(id.to_string());
            }
        }
        matcher.record_created(canonical, objects);
    }

    let mut updated = 0usize;
    for (server_id, body) in &to_update {
        updated += execute_update(jmap, canonical, server_id, body, &mut state.created_ids)?.0;
        if let Some(patch) = body.as_object() {
            matcher.record_updated(canonical, server_id, patch);
        }
    }

    for id in &matched_ids {
        state.touch(canonical, id);
    }

    Ok(UpsertOutcome {
        created,
        updated,
        matched_ids,
        scope,
    })
}

#[allow(clippy::too_many_arguments)]
fn execute_upsert(
    ctx: &Context,
    jmap: &Jmap,
    canonical: &str,
    match_on: Option<&MatchOn>,
    scope: Option<&Map<String, Value>>,
    value: &Map<String, Value>,
    state: &mut State,
    matcher: &mut Matcher,
    op_index: usize,
    reporter: &Reporter,
) -> CliResult<(usize, usize)> {
    let outcome = upsert_core(
        ctx, jmap, canonical, match_on, scope, value, state, matcher, op_index, reporter,
    )?;
    Ok((outcome.created, outcome.updated))
}

#[allow(clippy::too_many_arguments)]
fn execute_reconcile(
    ctx: &Context,
    jmap: &Jmap,
    canonical: &str,
    match_on: Option<&MatchOn>,
    scope: Option<&Map<String, Value>>,
    value: &Map<String, Value>,
    state: &mut State,
    matcher: &mut Matcher,
    op_index: usize,
    reporter: &Reporter,
) -> CliResult<(usize, usize, Vec<String>)> {
    let outcome = upsert_core(
        ctx, jmap, canonical, match_on, scope, value, state, matcher, op_index, reporter,
    )?;
    let leaked = leaked_ids(matcher, canonical, &outcome);
    Ok((outcome.created, outcome.updated, leaked))
}

fn resolve_scope(
    schema: &Schema,
    canonical: &str,
    filter: &Map<String, Value>,
    created_ids: &HashMap<String, String>,
) -> CliResult<Vec<(String, Value)>> {
    let mut resolved = Vec::with_capacity(filter.len());
    for (prop, raw) in filter {
        let value =
            match client_ref(raw).filter(|_| scope_prop_is_reference(schema, canonical, prop)) {
                Some(client_id) => match created_ids.get(client_id) {
                    Some(server_id) => Value::String(server_id.clone()),
                    None => {
                        return Err(CliError::msg(format!(
                            "{}: `scope` property `{}` references unresolved id `#{}` \
                         (no create, upsert, or reconcile operation in this plan produced it)",
                            resolve::display_name(canonical),
                            prop,
                            client_id,
                        )));
                    }
                },
                None => raw.clone(),
            };
        resolved.push((prop.clone(), value));
    }
    Ok(resolved)
}

fn scoped_body<'a>(
    body: &'a Map<String, Value>,
    scope: Option<&[(String, Value)]>,
) -> Cow<'a, Map<String, Value>> {
    let Some(props) = scope else {
        return Cow::Borrowed(body);
    };
    if props.iter().all(|(p, _)| body.contains_key(p)) {
        return Cow::Borrowed(body);
    }
    let mut merged = body.clone();
    for (p, wanted) in props {
        merged.entry(p.clone()).or_insert_with(|| wanted.clone());
    }
    Cow::Owned(merged)
}

fn in_scope(candidate: &Map<String, Value>, scope: Option<&[(String, Value)]>) -> bool {
    match scope {
        None => true,
        Some(props) => props
            .iter()
            .all(|(p, wanted)| values_match(candidate.get(p), Some(wanted))),
    }
}

fn values_match(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None | Some(Value::Null), None | Some(Value::Null)) => true,
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn client_ref(raw: &Value) -> Option<&str> {
    raw.as_str()
        .and_then(|s| s.strip_prefix('#'))
        .filter(|s| !s.is_empty())
}

fn scope_prop_is_reference(schema: &Schema, canonical: &str, prop: &str) -> bool {
    let is_ref = |fields: Option<&Fields>| {
        fields
            .and_then(|f| f.properties.get(prop))
            .is_some_and(|f| matches!(f.typ, FieldType::ObjectId { .. }))
    };
    match schema.schemas.get(canonical) {
        Some(ObjectSchema::Single { schema_name }) => is_ref(schema.fields.get(schema_name)),
        Some(ObjectSchema::Multiple { variants }) => variants
            .iter()
            .any(|v| is_ref(v.schema_name.as_ref().and_then(|sn| schema.fields.get(sn)))),
        None => false,
    }
}

fn leaked_ids(matcher: &Matcher, canonical: &str, outcome: &UpsertOutcome) -> Vec<String> {
    let mut leaked = Vec::new();
    for cand in matcher.objects_for(canonical) {
        let Some(id) = cand.get("id").and_then(Value::as_str) else {
            continue;
        };
        if outcome.matched_ids.contains(id) {
            continue;
        }
        if !in_scope(cand, outcome.scope.as_deref()) {
            continue;
        }
        leaked.push(id.to_string());
    }
    leaked
}

fn execute_update(
    jmap: &Jmap,
    canonical: &str,
    id: &str,
    value: &Value,
    created_ids: &mut HashMap<String, String>,
) -> CliResult<(usize, String)> {
    let real_id = if let Some(r) = id.strip_prefix('#')
        && !r.is_empty()
    {
        match created_ids.get(r) {
            Some(server_id) => server_id.clone(),
            None => {
                return Err(CliError::msg(format!(
                    "{}: update references unknown created id `#{}` (no matching create operation in this plan)",
                    resolve::display_name(canonical),
                    r,
                )));
            }
        }
    } else {
        id.to_string()
    };

    let mut refs: HashSet<String> = HashSet::new();
    collect_refs(value, &mut refs);

    let request_ids = request_created_ids(&refs, created_ids, &HashSet::new());
    let method = format!("{canonical}/set");
    let args = json!({ "update": { &real_id: value.clone() } });
    let call_resp = jmap.call_with(
        vec![(method.clone(), args, "c0".to_string())],
        if request_ids.is_empty() {
            None
        } else {
            Some(&request_ids)
        },
    )?;

    merge_created_ids(created_ids, call_resp.created_ids.as_ref());

    let result = check_single_response(call_resp, &method)?;
    if let Some(fails) = result.get("notUpdated").and_then(Value::as_object)
        && let Some((bad_id, err_val)) = fails.iter().next()
    {
        let set_err: SetError = serde_json::from_value(err_val.clone()).unwrap_or_default();
        return Err(CliError::msg(format!(
            "{}: update failed for id {}: {}",
            resolve::display_name(canonical),
            bad_id,
            set_error::render_with_refs(
                &set_err,
                Ansi::new(false),
                &reference_labels(&refs, created_ids)
            )
            .replace('\n', " | ")
        )));
    }
    let updated = result
        .get("updated")
        .and_then(Value::as_object)
        .map(|m| m.len())
        .unwrap_or(0);
    if updated == 0 {
        return Err(CliError::msg(format!(
            "{}: server returned no `updated` entry for id `{}` (the id may not exist)",
            resolve::display_name(canonical),
            real_id,
        )));
    }
    Ok((updated, real_id))
}

fn execute_create(
    jmap: &Jmap,
    canonical: &str,
    value: &Map<String, Value>,
    batch_size: usize,
    created_ids: &mut HashMap<String, String>,
    op_index: usize,
    reporter: &Reporter,
) -> CliResult<Vec<Map<String, Value>>> {
    let entries: Vec<(&String, &Value)> = value.iter().collect();
    let method = format!("{canonical}/set");
    let total = entries.len();
    let batches = total.div_ceil(batch_size);
    let mut created_objects: Vec<Map<String, Value>> = Vec::with_capacity(total);

    for (n, chunk) in entries.chunks(batch_size).enumerate() {
        let created_in_request: HashSet<String> = chunk.iter().map(|(k, _)| (*k).clone()).collect();

        let mut create_map = Map::new();
        let mut refs: HashSet<String> = HashSet::new();
        for (k, v) in chunk {
            collect_refs(v, &mut refs);
            create_map.insert((*k).clone(), (*v).clone());
        }
        let request_ids = request_created_ids(&refs, created_ids, &created_in_request);

        if reporter.progress {
            reporter.batch_note(&format!(
                "creating {} batch {}/{} ({} objects)",
                resolve::display_name(canonical),
                n + 1,
                batches,
                chunk.len()
            ));
        }

        let args = json!({ "create": create_map });
        let call_resp = jmap.call_with(
            vec![(method.clone(), args, "c0".to_string())],
            if request_ids.is_empty() {
                None
            } else {
                Some(&request_ids)
            },
        )?;
        merge_created_ids(created_ids, call_resp.created_ids.as_ref());

        let result = check_single_response(call_resp, &method)?;

        if let Some(fails) = result.get("notCreated").and_then(Value::as_object)
            && let Some((bad_id, err_val)) = fails.iter().next()
        {
            let set_err: SetError = serde_json::from_value(err_val.clone()).unwrap_or_default();
            return Err(CliError::msg(format!(
                "{}: create failed for `{}` (operation #{}): {}",
                resolve::display_name(canonical),
                bad_id,
                op_index + 1,
                set_error::render_with_refs(
                    &set_err,
                    Ansi::new(false),
                    &reference_labels(&refs, created_ids)
                )
                .replace('\n', " | ")
            )));
        }

        if let Some(created) = result.get("created").and_then(Value::as_object) {
            for (user_id, body) in created {
                let Some(server_id) = body.get("id").and_then(Value::as_str) else {
                    continue;
                };
                created_ids.insert(user_id.clone(), server_id.to_string());

                let mut obj = create_map
                    .get(user_id)
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                if let Some(assigned) = body.as_object() {
                    for (prop, v) in assigned {
                        obj.insert(prop.clone(), v.clone());
                    }
                }
                created_objects.push(obj);
            }
        }
    }
    Ok(created_objects)
}

fn check_single_response(resp: CallResponse, expected_name: &str) -> CliResult<Value> {
    if resp.responses.len() != 1 {
        return Err(CliError::UnexpectedResponse(format!(
            "expected 1 method response, got {}",
            resp.responses.len()
        )));
    }
    let (name, result, _) = resp
        .responses
        .into_iter()
        .next()
        .ok_or_else(|| CliError::UnexpectedResponse("empty methodResponses".into()))?;
    if name == "error" {
        return Err(crate::jmap::protocol::jmap_error_from_value(&result));
    }
    if name != expected_name {
        return Err(CliError::UnexpectedResponse(format!(
            "expected response `{expected_name}`, got `{name}`"
        )));
    }
    Ok(result)
}

fn merge_created_ids(dst: &mut HashMap<String, String>, src: Option<&HashMap<String, String>>) {
    let Some(map) = src else { return };
    for (k, v) in map {
        dst.insert(k.clone(), v.clone());
    }
}

struct Reporter {
    quiet: bool,
    json: bool,
    progress: bool,
    ansi: Ansi,
}

impl Reporter {
    fn new(args: &ApplyArgs, ansi: Ansi) -> Self {
        Reporter {
            quiet: args.quiet,
            json: args.json,
            progress: args.progress,
            ansi,
        }
    }

    fn plan_header(&self, plan: &Plan) {
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = writeln!(
            err,
            "{}Plan:{} {} destroy, {} update, {} create, {} upsert, {} reconcile ({} objects)",
            self.ansi.bold(),
            self.ansi.reset(),
            plan.destroys,
            plan.updates,
            plan.creates,
            plan.upserts,
            plan.reconciles,
            plan.create_objects + plan.upsert_objects + plan.reconcile_objects,
        );
    }

    fn value_match_warning(&self, canonical: &str) {
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = writeln!(
            err,
            "{}warning:{} {} has no match key; matching existing objects by value \
             (a changed object will be created as a new one)",
            self.ansi.yellow(),
            self.ansi.reset(),
            resolve::display_name(canonical),
        );
    }

    fn dry_run_note(&self) {
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "(dry run: no changes will be made)");
    }

    fn batch_note(&self, msg: &str) {
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{}·{} {msg}", self.ansi.dim(), self.ansi.reset());
    }

    fn op_ok(&self, index: usize, kind: &str, canonical: &str, count: usize) {
        let disp = resolve::display_name(canonical);
        if self.json {
            let line = json!({
                "op": kind,
                "object": disp,
                "index": index,
                "count": count,
                "status": "ok",
            });
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{}", line);
            return;
        }
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = writeln!(
            err,
            "{}✓{} {} {} ({})",
            self.ansi.green(),
            self.ansi.reset(),
            past_tense(kind),
            disp,
            count
        );
    }

    fn reconcile_cleanup(&self, index: usize, canonical: &str, count: usize) {
        let disp = resolve::display_name(canonical);
        if self.json {
            let line = json!({
                "op": "reconcile",
                "stage": "cleanup",
                "object": disp,
                "index": index,
                "destroyed": count,
                "status": "ok",
            });
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{}", line);
            return;
        }
        if self.quiet || count == 0 {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = writeln!(
            err,
            "{}✓{} reconcile removed {} ({})",
            self.ansi.green(),
            self.ansi.reset(),
            disp,
            count
        );
    }

    fn op_err(&self, index: usize, kind: &str, canonical: &str, err: &CliError) {
        let disp = resolve::display_name(canonical);
        if self.json {
            let line = json!({
                "op": kind,
                "object": disp,
                "index": index,
                "status": "error",
                "error": err.to_string(),
            });
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{}", line);
            return;
        }
        let mut errio = std::io::stderr().lock();
        let _ = writeln!(
            errio,
            "{}✗{} {} {}: {err}",
            self.ansi.red(),
            self.ansi.reset(),
            kind,
            disp
        );
    }

    fn final_summary(&self, s: &Summary) {
        if self.json {
            let line = json!({
                "op": "summary",
                "plan": {
                    "destroys": s.planned_destroys,
                    "updates": s.planned_updates,
                    "creates": s.planned_creates,
                    "create_objects": s.planned_create_objects,
                    "upserts": s.planned_upserts,
                    "upsert_objects": s.planned_upsert_objects,
                    "reconciles": s.planned_reconciles,
                    "reconcile_objects": s.planned_reconcile_objects,
                },
                "done": {
                    "destroyed": s.destroyed,
                    "updated": s.updated,
                    "created": s.created,
                    "failed": s.failed,
                },
            });
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{}", line);
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = writeln!(
            err,
            "{}Done:{} {} destroyed, {} updated, {} created ({} failed)",
            self.ansi.bold(),
            self.ansi.reset(),
            s.destroyed,
            s.updated,
            s.created,
            s.failed,
        );
    }
}

fn past_tense(kind: &str) -> &'static str {
    match kind {
        "destroy" => "destroyed",
        "update" => "updated",
        "create" => "created",
        "upsert" => "upserted",
        "reconcile" => "reconciled",
        _ => "done",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_labels_map_server_ids_back_to_plan_refs() {
        let mut refs = HashSet::new();
        refs.insert("dnsserver-ovh".to_string());
        refs.insert("tenant-a".to_string());
        refs.insert("never-created".to_string());
        let state_ids: HashMap<String, String> = [
            ("dnsserver-ovh".to_string(), "i1nk7i22boqc".to_string()),
            ("tenant-a".to_string(), "a4bc9".to_string()),
            ("unreferenced".to_string(), "zzz".to_string()),
        ]
        .into_iter()
        .collect();

        let labels = reference_labels(&refs, &state_ids);
        assert_eq!(labels.get("i1nk7i22boqc"), Some(&"dnsserver-ovh"));
        assert_eq!(labels.get("a4bc9"), Some(&"tenant-a"));
        assert_eq!(labels.get("zzz"), None);
    }

    #[test]
    fn reference_labels_drop_ids_shared_by_two_refs() {
        let mut refs = HashSet::new();
        refs.insert("dns-a".to_string());
        refs.insert("tenant-a".to_string());
        let state_ids: HashMap<String, String> = [
            ("dns-a".to_string(), "same".to_string()),
            ("tenant-a".to_string(), "same".to_string()),
        ]
        .into_iter()
        .collect();

        assert!(reference_labels(&refs, &state_ids).is_empty());
    }

    fn lookup_key_schema() -> Schema {
        use crate::schema::Fields;
        let mut s = Schema::default();
        s.objects.insert("x:MemoryLookupKey".into(), obj_type());
        s.schemas.insert(
            "x:MemoryLookupKey".into(),
            ObjectSchema::Single {
                schema_name: "x:MemoryLookupKey".into(),
            },
        );
        let mut props = HashMap::new();
        props.insert("namespace".to_string(), string_field());
        props.insert("key".to_string(), string_field());
        s.fields.insert(
            "x:MemoryLookupKey".into(),
            Fields {
                properties: props,
                defaults: HashMap::new(),
            },
        );
        s
    }

    fn server_set_scope_schema() -> Schema {
        use crate::schema::Fields;
        let mut s = Schema::default();
        s.objects.insert("x:Cert".into(), obj_type());
        s.schemas.insert(
            "x:Cert".into(),
            ObjectSchema::Single {
                schema_name: "x:Cert".into(),
            },
        );
        let mut fingerprint = string_field();
        fingerprint.update = crate::schema::FieldUpdate::ServerSet;
        let mut props = HashMap::new();
        props.insert("name".to_string(), string_field());
        props.insert("fingerprint".to_string(), fingerprint);
        s.fields.insert(
            "x:Cert".into(),
            Fields {
                properties: props,
                defaults: HashMap::new(),
            },
        );
        s
    }

    fn entries_of<'a>(
        value: &'a Map<String, Value>,
        scope: Option<&[(String, Value)]>,
    ) -> Vec<(&'a String, std::borrow::Cow<'a, Map<String, Value>>)> {
        value
            .iter()
            .map(|(k, v)| (k, scoped_body(v.as_object().unwrap(), scope)))
            .collect()
    }

    #[test]
    fn collects_string_refs() {
        let v = json!({ "a": "#x", "b": ["plain", "#y"], "c": "no ref" });
        let mut out = HashSet::new();
        collect_refs(&v, &mut out);
        assert_eq!(out, ["x", "y"].iter().map(|s| s.to_string()).collect());
    }

    #[test]
    fn collects_key_refs() {
        let v = json!({ "roleIds": { "#id1": true, "#id2": true, "plain": false } });
        let mut out = HashSet::new();
        collect_refs(&v, &mut out);
        assert_eq!(out, ["id1", "id2"].iter().map(|s| s.to_string()).collect());
    }

    #[test]
    fn request_ids_excludes_in_flight() {
        let refs: HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let state: HashMap<String, String> = [("a", "sa"), ("b", "sb")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let in_flight: HashSet<String> = ["a"].iter().map(|s| s.to_string()).collect();
        let got = request_created_ids(&refs, &state, &in_flight);
        assert_eq!(got.get("a"), None);
        assert_eq!(got.get("b").map(String::as_str), Some("sb"));
    }

    #[test]
    fn parses_three_ndjson_records() {
        let input = "{\"@type\":\"destroy\",\"object\":\"Domain\"}\n\
                     {\"@type\":\"create\",\"object\":\"Domain\",\"value\":{\"d1\":{\"name\":\"a\"}}}\n\
                     {\"@type\":\"update\",\"object\":\"DataStore\",\"id\":\"singleton\",\"value\":{}}\n";
        let ops = parse_ndjson_plan(input).unwrap();
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[0], RawOp::Destroy { .. }));
        assert!(matches!(ops[1], RawOp::Create { .. }));
        assert!(matches!(ops[2], RawOp::Update { .. }));
    }

    #[test]
    fn skips_blank_lines_and_trailing_newline() {
        let input = "\n  \n{\"@type\":\"destroy\",\"object\":\"Domain\"}\n\n";
        let ops = parse_ndjson_plan(input).unwrap();
        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn empty_input_yields_no_ops() {
        let ops = parse_ndjson_plan("").unwrap();
        assert!(ops.is_empty());
        let ops = parse_ndjson_plan("\n  \n\n").unwrap();
        assert!(ops.is_empty());
    }

    #[test]
    fn invalid_line_reports_line_number() {
        let input = "{\"@type\":\"destroy\",\"object\":\"Domain\"}\n\
                     not json at all\n";
        let err = parse_ndjson_plan(input).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("line 2"), "expected line 2, got: {msg}");
    }

    #[test]
    fn missing_update_id_names_top_level_field() {
        let err = resolve_update_id("x:Domain", false, None, 4).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("operation #5"), "expected op #5, got: {msg}");
        assert!(msg.contains("top-level `id`"), "missing field hint: {msg}");
        assert!(
            msg.contains("sibling of `value`"),
            "missing shape hint: {msg}"
        );
    }

    #[test]
    fn missing_update_id_is_ok_for_singleton() {
        let id = resolve_update_id("x:SystemSettings", true, None, 0).unwrap();
        assert_eq!(id, "singleton");
        let id = resolve_update_id("x:SystemSettings", true, Some("singleton"), 0).unwrap();
        assert_eq!(id, "singleton");
    }

    #[test]
    fn rejects_json_array_form() {
        let input = "[{\"@type\":\"destroy\",\"object\":\"Domain\"}]\n";
        let err = parse_ndjson_plan(input).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("line 1"));
    }

    fn obj_type() -> ObjectType {
        ObjectType::Object {
            description: String::new(),
            permission_prefix: String::new(),
            enterprise: false,
        }
    }

    fn string_field() -> Field {
        Field {
            description: String::new(),
            typ: FieldType::String {
                format: StringFormat::String,
                min_length: None,
                max_length: None,
                nullable: false,
            },
            update: crate::schema::FieldUpdate::Mutable,
            enterprise: false,
        }
    }

    fn domain_schema() -> Schema {
        use crate::schema::{Fields, List};
        let mut s = Schema::default();
        s.objects.insert("x:Domain".into(), obj_type());
        s.schemas.insert(
            "x:Domain".into(),
            ObjectSchema::Single {
                schema_name: "x:Domain".into(),
            },
        );
        let mut props = HashMap::new();
        props.insert("name".to_string(), string_field());
        props.insert("description".to_string(), string_field());
        s.fields.insert(
            "x:Domain".into(),
            Fields {
                properties: props,
                defaults: HashMap::new(),
            },
        );
        s.lists.insert(
            "x:Domain".into(),
            List {
                title: String::new(),
                subtitle: String::new(),
                label_property: Some("name".into()),
                singular_name: String::new(),
                plural_name: String::new(),
                columns: vec![],
                filters: vec![],
                filters_static: HashMap::new(),
                sort: vec![],
                mass_actions: vec![],
                item_actions: vec![],
            },
        );
        s
    }

    fn matcher_with(objs: Vec<Value>) -> Matcher {
        matcher_for("x:Domain", objs)
    }

    fn matcher_for(canonical: &str, objs: Vec<Value>) -> Matcher {
        let mut m = Matcher::new();
        m.objects.insert(
            canonical.into(),
            objs.into_iter()
                .filter_map(|v| v.as_object().cloned())
                .collect(),
        );
        m
    }

    fn object_id_field(object_name: &str) -> Field {
        Field {
            description: String::new(),
            typ: FieldType::ObjectId {
                object_name: object_name.into(),
                nullable: false,
            },
            update: crate::schema::FieldUpdate::Mutable,
            enterprise: false,
        }
    }

    fn account_schema() -> Schema {
        use crate::schema::Fields;
        let mut s = Schema::default();
        s.objects.insert("x:Account".into(), obj_type());
        s.schemas.insert(
            "x:Account".into(),
            ObjectSchema::Single {
                schema_name: "x:Account".into(),
            },
        );
        let mut props = HashMap::new();
        props.insert("name".to_string(), string_field());
        props.insert("domainId".to_string(), object_id_field("x:Domain"));
        s.fields.insert(
            "x:Account".into(),
            Fields {
                properties: props,
                defaults: HashMap::new(),
            },
        );
        s
    }

    #[test]
    fn parses_upsert_op_with_match_on() {
        let input = "{\"@type\":\"upsert\",\"object\":\"Domain\",\"matchOn\":[\"name\"],\
                     \"value\":{\"d1\":{\"name\":\"a.com\"}}}\n";
        let ops = parse_ndjson_plan(input).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            RawOp::Upsert {
                object,
                match_on,
                value,
                ..
            } => {
                assert_eq!(object, "Domain");
                assert!(matches!(match_on, Some(MatchOn::Props(p)) if p == &["name".to_string()]));
                assert!(value.contains_key("d1"));
            }
            other => panic!("expected upsert, got {other:?}"),
        }
    }

    #[test]
    fn resolve_match_key_prefers_match_on_then_label_then_value() {
        let s = domain_schema();
        assert!(matches!(
            resolve_match_key(&s, "x:Domain", Some(&MatchOn::Props(vec!["description".to_string()]))),
            MatchKey::Props(p) if p == ["description"]
        ));
        assert!(matches!(
            resolve_match_key(&s, "x:Domain", None),
            MatchKey::Props(p) if p == ["name"]
        ));
        let mut bare = Schema::default();
        bare.objects.insert("x:Tracer".into(), obj_type());
        assert!(matches!(
            resolve_match_key(&bare, "x:Tracer", None),
            MatchKey::Value
        ));
    }

    #[test]
    fn find_match_by_label_matches_unique() {
        let s = domain_schema();
        let m = matcher_with(vec![
            json!({ "id": "srv1", "name": "a.com" }),
            json!({ "id": "srv2", "name": "b.com" }),
        ]);
        let body = json!({ "name": "b.com" }).as_object().unwrap().clone();
        let key = resolve_match_key(&s, "x:Domain", None);
        let got = find_match(&m, &s, "x:Domain", &body, &key, None, &HashMap::new()).unwrap();
        assert_eq!(got.as_deref(), Some("srv2"));

        let body = json!({ "name": "nope.com" }).as_object().unwrap().clone();
        let got = find_match(&m, &s, "x:Domain", &body, &key, None, &HashMap::new()).unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn find_match_ambiguous_is_error() {
        let s = domain_schema();
        let m = matcher_with(vec![
            json!({ "id": "srv1", "name": "dup.com" }),
            json!({ "id": "srv2", "name": "dup.com" }),
        ]);
        let body = json!({ "name": "dup.com" }).as_object().unwrap().clone();
        let key = resolve_match_key(&s, "x:Domain", None);
        let err = find_match(&m, &s, "x:Domain", &body, &key, None, &HashMap::new()).unwrap_err();
        assert!(format!("{err}").contains("ambiguous"));
    }

    #[test]
    fn find_match_missing_key_property_is_error() {
        let s = domain_schema();
        let m = matcher_with(vec![json!({ "id": "srv1", "name": "a.com" })]);
        let body = json!({ "description": "x" }).as_object().unwrap().clone();
        let key = resolve_match_key(&s, "x:Domain", None);
        let err = find_match(&m, &s, "x:Domain", &body, &key, None, &HashMap::new()).unwrap_err();
        assert!(format!("{err}").contains("match property `name`"));
    }

    #[test]
    fn find_match_value_fallback_matches_on_scalars() {
        let mut s = domain_schema();
        s.lists.get_mut("x:Domain").unwrap().label_property = None;
        let m = matcher_with(vec![
            json!({ "id": "srv1", "name": "a.com", "description": "one" }),
            json!({ "id": "srv2", "name": "b.com", "description": "two" }),
        ]);
        let key = resolve_match_key(&s, "x:Domain", None);
        assert!(matches!(key, MatchKey::Value));
        let body = json!({ "name": "b.com", "description": "two" })
            .as_object()
            .unwrap()
            .clone();
        let got = find_match(&m, &s, "x:Domain", &body, &key, None, &HashMap::new()).unwrap();
        assert_eq!(got.as_deref(), Some("srv2"));

        let changed = json!({ "name": "b.com", "description": "CHANGED" })
            .as_object()
            .unwrap()
            .clone();
        let got = find_match(&m, &s, "x:Domain", &changed, &key, None, &HashMap::new()).unwrap();
        assert_eq!(got, None, "a changed scalar must not match (creates new)");
    }

    #[test]
    fn upsert_singleton_is_rejected() {
        let mut s = Schema::default();
        s.objects.insert(
            "x:SystemSettings".into(),
            ObjectType::Singleton {
                description: String::new(),
                permission_prefix: String::new(),
                enterprise: false,
            },
        );
        let raw = vec![RawOp::Upsert {
            object: "SystemSettings".into(),
            match_on: None,
            scope: None,
            value: {
                let mut m = Map::new();
                m.insert("s1".into(), json!({ "x": 1 }));
                m
            },
        }];
        let err = match Plan::resolve(&s, raw) {
            Ok(_) => panic!("expected an error for upsert on a singleton"),
            Err(e) => e,
        };
        assert!(format!("{err}").contains("cannot upsert singleton"));
    }

    fn account_singleton_settings() -> Schema {
        let mut s = Schema::default();
        s.objects.insert(
            "x:SystemSettings".into(),
            ObjectType::Singleton {
                description: String::new(),
                permission_prefix: String::new(),
                enterprise: false,
            },
        );
        s
    }

    #[test]
    fn resolve_match_value_substitutes_object_id_ref() {
        let s = account_schema();
        let fields = s.fields.get("x:Account");
        let ids: HashMap<String, String> = [("dom".to_string(), "srvD".to_string())]
            .into_iter()
            .collect();
        let got =
            resolve_match_value("x:Account", fields, "domainId", &json!("#dom"), &ids).unwrap();
        assert_eq!(
            got,
            json!("srvD"),
            "an ObjectId ref must resolve to the server id"
        );
    }

    #[test]
    fn resolve_match_value_leaves_non_reference_fields_untouched() {
        let s = account_schema();
        let fields = s.fields.get("x:Account");
        let ids = HashMap::new();
        let got = resolve_match_value("x:Account", fields, "name", &json!("#dom"), &ids).unwrap();
        assert_eq!(
            got,
            json!("#dom"),
            "a `#`-looking value in a non-ObjectId field is not a reference"
        );
    }

    #[test]
    fn resolve_match_value_unresolved_ref_is_error() {
        let s = account_schema();
        let fields = s.fields.get("x:Account");
        let err = resolve_match_value(
            "x:Account",
            fields,
            "domainId",
            &json!("#missing"),
            &HashMap::new(),
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("references unresolved id `#missing`"),
            "expected an unresolved-ref error, got: {msg}"
        );
    }

    #[test]
    fn resolve_match_value_literal_id_passes_through() {
        let s = account_schema();
        let fields = s.fields.get("x:Account");
        let got = resolve_match_value(
            "x:Account",
            fields,
            "domainId",
            &json!("srvD"),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            got,
            json!("srvD"),
            "a bare (non-#) id must not be looked up"
        );
    }

    #[test]
    fn find_match_resolves_ref_in_compound_key() {
        let s = account_schema();
        let m = matcher_for(
            "x:Account",
            vec![
                json!({ "id": "a1", "name": "sales", "domainId": "srvA" }),
                json!({ "id": "a2", "name": "sales", "domainId": "srvB" }),
            ],
        );
        let ids: HashMap<String, String> = [("dom-b".to_string(), "srvB".to_string())]
            .into_iter()
            .collect();
        let body = json!({ "name": "sales", "domainId": "#dom-b" })
            .as_object()
            .unwrap()
            .clone();
        let key = MatchKey::Props(vec!["name".into(), "domainId".into()]);
        let got = find_match(&m, &s, "x:Account", &body, &key, None, &ids).unwrap();
        assert_eq!(
            got.as_deref(),
            Some("a2"),
            "the effective primary key (name + resolved domainId) must select a2"
        );
    }

    #[test]
    fn find_match_unresolved_ref_in_key_is_error() {
        let s = account_schema();
        let m = matcher_for(
            "x:Account",
            vec![json!({ "id": "a1", "name": "sales", "domainId": "srvA" })],
        );
        let body = json!({ "name": "sales", "domainId": "#dom-x" })
            .as_object()
            .unwrap()
            .clone();
        let key = MatchKey::Props(vec!["name".into(), "domainId".into()]);
        let err = find_match(&m, &s, "x:Account", &body, &key, None, &HashMap::new()).unwrap_err();
        assert!(format!("{err}").contains("references unresolved id `#dom-x`"));
    }

    #[test]
    fn find_match_hash_literal_in_text_prop_is_not_a_ref() {
        let s = account_schema();
        let m = matcher_for(
            "x:Account",
            vec![json!({ "id": "a1", "name": "#literal", "domainId": "srvA" })],
        );
        let body = json!({ "name": "#literal" }).as_object().unwrap().clone();
        let key = MatchKey::Props(vec!["name".into()]);
        let got = find_match(&m, &s, "x:Account", &body, &key, None, &HashMap::new()).unwrap();
        assert_eq!(
            got.as_deref(),
            Some("a1"),
            "a literal `#` in a String field must compare as-is, not resolve"
        );
    }

    #[test]
    fn parses_reconcile_op() {
        let input = "{\"@type\":\"reconcile\",\"object\":\"Domain\",\"matchOn\":[\"name\"],\
                     \"value\":{\"d1\":{\"name\":\"a.com\"}}}\n";
        let ops = parse_ndjson_plan(input).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], RawOp::Reconcile { .. }));
    }

    #[test]
    fn reconcile_allows_empty_value() {
        let s = domain_schema();
        let raw = vec![RawOp::Reconcile {
            object: "Domain".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: None,
            value: Map::new(),
        }];
        let plan =
            Plan::resolve(&s, raw).expect("empty reconcile value means delete-all, not an error");
        assert_eq!(plan.reconciles, 1);
        assert_eq!(plan.reconcile_objects, 0);
    }

    #[test]
    fn reconcile_singleton_is_rejected() {
        let s = account_singleton_settings();
        let raw = vec![RawOp::Reconcile {
            object: "SystemSettings".into(),
            match_on: None,
            scope: None,
            value: {
                let mut m = Map::new();
                m.insert("s1".into(), json!({ "x": 1 }));
                m
            },
        }];
        let err = match Plan::resolve(&s, raw) {
            Ok(_) => panic!("expected an error for reconcile on a singleton"),
            Err(e) => e,
        };
        assert!(format!("{err}").contains("cannot reconcile singleton"));
    }

    #[test]
    fn reconcile_empty_match_on_is_rejected() {
        let s = domain_schema();
        let raw = vec![RawOp::Reconcile {
            object: "Domain".into(),
            match_on: Some(MatchOn::Props(vec![])),
            scope: None,
            value: {
                let mut m = Map::new();
                m.insert("d1".into(), json!({ "name": "a.com" }));
                m
            },
        }];
        let err = match Plan::resolve(&s, raw) {
            Ok(_) => panic!("expected an error for reconcile with an empty matchOn"),
            Err(e) => e,
        };
        assert!(format!("{err}").contains("empty `matchOn`"));
    }

    #[test]
    fn leaked_ids_single_variant_flags_unmatched() {
        let m = matcher_with(vec![
            json!({ "id": "srv1", "name": "keep.com" }),
            json!({ "id": "srv2", "name": "gone.com" }),
        ]);
        let outcome = UpsertOutcome {
            created: 0,
            updated: 1,
            matched_ids: ["srv1".to_string()].into_iter().collect(),
            scope: None,
        };
        let leaked = leaked_ids(&m, "x:Domain", &outcome);
        assert_eq!(
            leaked,
            vec!["srv2".to_string()],
            "unmatched objects are leaked"
        );
    }

    #[test]
    fn leaked_ids_empty_plan_flags_everything() {
        let m = matcher_with(vec![
            json!({ "id": "srv1", "name": "a.com" }),
            json!({ "id": "srv2", "name": "b.com" }),
        ]);
        let outcome = UpsertOutcome {
            created: 0,
            updated: 0,
            matched_ids: HashSet::new(),
            scope: None,
        };
        let mut leaked = leaked_ids(&m, "x:Domain", &outcome);
        leaked.sort();
        assert_eq!(
            leaked,
            vec!["srv1".to_string(), "srv2".to_string()],
            "an empty single-variant reconcile deletes all existing objects"
        );
    }

    fn mta_route_multi_schema_with_fields() -> Schema {
        use crate::schema::Fields;
        let mut s = mta_route_multi_schema();
        for name in ["x:MtaRouteLocal", "x:MtaRouteMx"] {
            let mut props = HashMap::new();
            props.insert("name".to_string(), string_field());
            s.fields.insert(
                name.into(),
                Fields {
                    properties: props,
                    defaults: HashMap::new(),
                },
            );
        }
        s
    }

    fn mta_route_multi_schema() -> Schema {
        let mut s = Schema::default();
        s.objects.insert("x:MtaRoute".into(), obj_type());
        s.schemas.insert(
            "x:MtaRoute".into(),
            ObjectSchema::Multiple {
                variants: vec![
                    crate::schema::ObjectVariant {
                        name: "Local".into(),
                        label: String::new(),
                        schema_name: Some("x:MtaRouteLocal".into()),
                    },
                    crate::schema::ObjectVariant {
                        name: "Mx".into(),
                        label: String::new(),
                        schema_name: Some("x:MtaRouteMx".into()),
                    },
                ],
            },
        );
        s
    }

    #[test]
    fn validate_plan_references_rejects_undeclared_matchon_ref() {
        let s = account_schema();
        let raw = vec![RawOp::Upsert {
            object: "Account".into(),
            match_on: Some(MatchOn::Props(vec!["domainId".into()])),
            scope: None,
            value: {
                let mut m = Map::new();
                m.insert(
                    "acc".into(),
                    json!({ "@type": "User", "domainId": "#ghost" }),
                );
                m
            },
        }];
        let plan = Plan::resolve(&s, raw).expect("plan resolves structurally");
        let err = validate_plan_references(&s, &plan).unwrap_err();
        assert!(
            format!("{err}").contains("unresolved id `#ghost`"),
            "an undeclared matchOn reference must be rejected upfront (dry-run safe)"
        );
    }

    #[test]
    fn validate_plan_references_accepts_declared_ref() {
        let s = account_schema();
        let raw = vec![
            RawOp::Create {
                scope: None,
                object: "Account".into(),
                value: {
                    let mut m = Map::new();
                    m.insert("dom".into(), json!({ "@type": "User", "name": "x" }));
                    m
                },
            },
            RawOp::Upsert {
                object: "Account".into(),
                match_on: Some(MatchOn::Props(vec!["domainId".into()])),
                scope: None,
                value: {
                    let mut m = Map::new();
                    m.insert("acc".into(), json!({ "@type": "User", "domainId": "#dom" }));
                    m
                },
            },
        ];
        let plan = Plan::resolve(&s, raw).expect("plan resolves structurally");
        validate_plan_references(&s, &plan)
            .expect("a reference produced by another op must validate");
    }

    #[test]
    fn validate_plan_references_rejects_out_of_order_ref() {
        let s = account_schema();
        let raw = vec![
            RawOp::Upsert {
                object: "Account".into(),
                match_on: Some(MatchOn::Props(vec!["domainId".into()])),
                scope: None,
                value: {
                    let mut m = Map::new();
                    m.insert("acc".into(), json!({ "@type": "User", "domainId": "#dom" }));
                    m
                },
            },
            RawOp::Create {
                scope: None,
                object: "Account".into(),
                value: {
                    let mut m = Map::new();
                    m.insert("dom".into(), json!({ "@type": "User", "name": "x" }));
                    m
                },
            },
        ];
        let plan = Plan::resolve(&s, raw).expect("plan resolves structurally");
        let err = validate_plan_references(&s, &plan).unwrap_err();
        assert!(
            format!("{err}").contains("unresolved id `#dom`"),
            "a matchOn ref to a LATER op must be rejected (dry-run must match runtime order)"
        );
    }

    #[test]
    fn resolve_match_value_null_and_missing_field() {
        let s = account_schema();
        let fields = s.fields.get("x:Account");
        let got = resolve_match_value(
            "x:Account",
            fields,
            "domainId",
            &Value::Null,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(got, Value::Null, "a null ObjectId value must pass through");
        let got = resolve_match_value(
            "x:Account",
            None,
            "domainId",
            &json!("#dom"),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            got,
            json!("#dom"),
            "with no field metadata the value is not treated as a reference"
        );
    }

    #[test]
    fn reconcile_without_match_key_is_rejected() {
        let mut s = Schema::default();
        s.objects.insert("x:Tracer".into(), obj_type());
        let raw = vec![RawOp::Reconcile {
            object: "Tracer".into(),
            match_on: None,
            scope: None,
            value: {
                let mut m = Map::new();
                m.insert("t1".into(), json!({ "name": "x" }));
                m
            },
        }];
        let err = match Plan::resolve(&s, raw) {
            Ok(_) => panic!("expected reconcile without a match key to be rejected"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("no match key") && msg.contains("\"matchOn\": \"*\""),
            "expected a value-fallback rejection pointing at the opt-in: {msg}"
        );
    }

    #[test]
    fn value_match_treats_an_absent_property_as_null() {
        let s = domain_schema();
        let m = matcher_with(vec![json!({
            "id": "srv1",
            "name": "a.com",
            "description": Value::Null,
        })]);
        let body = json!({ "name": "a.com" }).as_object().unwrap().clone();
        let got = find_match(
            &m,
            &s,
            "x:Domain",
            &body,
            &MatchKey::Value,
            None,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            got.as_deref(),
            Some("srv1"),
            "a server-returned null must compare equal to a property the plan body omits, \
             otherwise re-applying an unchanged plan looks like drift"
        );
    }

    #[test]
    fn value_match_still_sees_a_real_difference() {
        let s = domain_schema();
        let m = matcher_with(vec![json!({
            "id": "srv1",
            "name": "a.com",
            "description": "set",
        })]);
        let body = json!({ "name": "a.com" }).as_object().unwrap().clone();
        let got = find_match(
            &m,
            &s,
            "x:Domain",
            &body,
            &MatchKey::Value,
            None,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            got, None,
            "a populated property the body omits is real drift, not a null"
        );
    }

    #[test]
    fn touched_ids_are_scoped_by_object_type() {
        let mut state = State {
            created_ids: HashMap::new(),
            touched: HashMap::new(),
            summary: Summary {
                planned_destroys: 0,
                planned_updates: 0,
                planned_creates: 0,
                planned_create_objects: 0,
                planned_upserts: 0,
                planned_upsert_objects: 0,
                planned_reconciles: 0,
                planned_reconcile_objects: 0,
                destroyed: 0,
                updated: 0,
                created: 0,
                failed: 0,
            },
        };
        state.touch("x:Domain", "b");
        assert!(state.is_touched("x:Domain", "b"));
        assert!(
            !state.is_touched("x:Account", "b"),
            "ids are only unique within a type; a matched Domain must not shield an Account \
             that happens to share its id from a reconcile's cleanup"
        );
    }

    #[test]
    fn a_misspelled_top_level_key_is_rejected() {
        let input = "{\"@type\":\"reconcile\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\
                     \"scoep\":{\"@type\":\"Local\"},\"value\":{}}\n";
        let err = match parse_ndjson_plan(input) {
            Ok(_) => panic!("expected an unknown key to be rejected"),
            Err(e) => format!("{e}"),
        };
        assert!(
            err.contains("scoep"),
            "a misspelled `scope` would silently widen a reconcile to the whole type: {err}"
        );
    }

    #[test]
    fn scope_on_a_non_variant_at_type_is_rejected() {
        let s = mta_route_multi_schema_with_fields();
        let raw = vec![RawOp::Reconcile {
            object: "MtaRoute".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!({ "@type": "Loacl" })),
            value: Map::new(),
        }];
        let err = match Plan::resolve(&s, raw) {
            Ok(_) => panic!("expected a bogus @type scope to be rejected"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("not a variant") && msg.contains("Local"),
            "a typo'd variant name selects nothing, which is indistinguishable from a \
             successful reconcile: {msg}"
        );
    }

    #[test]
    fn scope_on_at_type_requires_a_multi_variant_type() {
        let s = domain_schema();
        let raw = vec![RawOp::Reconcile {
            object: "Domain".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!({ "@type": "Local" })),
            value: Map::new(),
        }];
        let err = match Plan::resolve(&s, raw) {
            Ok(_) => panic!("expected an @type scope on a single-variant type to be rejected"),
            Err(e) => e,
        };
        assert!(format!("{err}").contains("not a multi-variant type"));
    }

    #[test]
    fn an_entry_inherits_the_scope_it_omits() {
        let scope = vec![("namespace".to_string(), json!("traps"))];
        let body = json!({ "key": "foo@*" }).as_object().unwrap().clone();
        let merged = scoped_body(&body, Some(&scope));
        assert_eq!(
            merged.get("namespace"),
            Some(&json!("traps")),
            "an omitted scope property must be filled in, or the created object lands outside \
             the scope and is created again on every apply"
        );
        assert_eq!(merged.get("key"), Some(&json!("foo@*")));
    }

    #[test]
    fn an_entry_keeps_its_own_value_for_a_scoped_property() {
        let scope = vec![("namespace".to_string(), json!("traps"))];
        let body = json!({ "key": "foo@*", "namespace": "traps" })
            .as_object()
            .unwrap()
            .clone();
        let merged = scoped_body(&body, Some(&scope));
        assert!(
            matches!(merged, std::borrow::Cow::Borrowed(_)),
            "a body that already sets every scope property must not be cloned"
        );
    }

    #[test]
    fn value_matching_rejects_two_identical_entries() {
        let s = domain_schema();
        let mut value = Map::new();
        value.insert("a".into(), json!({ "name": "dup.com" }));
        value.insert("b".into(), json!({ "name": "dup.com" }));
        let err = match reject_duplicate_match_keys(
            &s,
            "x:Domain",
            &entries_of(&value, None),
            &MatchKey::Value,
            &HashMap::new(),
            0,
        ) {
            Ok(_) => panic!("expected two identical entries to be rejected under value matching"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("identical values"),
            "both entries would be created, and every later apply would fail as ambiguous: {msg}"
        );
    }

    #[test]
    fn scope_reference_and_literal_id_are_not_a_contradiction() {
        let s = account_schema();
        let raw = vec![RawOp::Upsert {
            object: "Account".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!({ "domainId": "#dom-a" })),
            value: {
                let mut m = Map::new();
                m.insert("acc".into(), json!({ "name": "a", "domainId": "srv-dom" }));
                m
            },
        }];
        Plan::resolve(&s, raw).expect(
            "a `#ref` and a literal id may denote the same object; only the run time knows, \
             so plan time must not call it a contradiction",
        );
    }

    #[test]
    fn an_entry_outside_its_own_scope_is_rejected() {
        let s = account_schema();
        let raw = vec![RawOp::Upsert {
            object: "Account".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!({ "domainId": "#dom-a" })),
            value: {
                let mut m = Map::new();
                m.insert("in".into(), json!({ "name": "a", "domainId": "#dom-a" }));
                m.insert("out".into(), json!({ "name": "b", "domainId": "#dom-b" }));
                m
            },
        }];
        let err = match Plan::resolve(&s, raw) {
            Ok(_) => panic!("expected an out-of-scope entry to be rejected"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("outside its own `scope`") && msg.contains("out"),
            "an out-of-scope entry can never match, so it would be recreated on every apply: \
             {msg}"
        );
    }

    #[test]
    fn an_entry_that_omits_a_scoped_property_is_allowed() {
        let s = account_schema();
        let raw = vec![RawOp::Upsert {
            object: "Account".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!({ "domainId": "#dom-a" })),
            value: {
                let mut m = Map::new();
                m.insert("a".into(), json!({ "name": "a" }));
                m
            },
        }];
        Plan::resolve(&s, raw).expect(
            "a body that omits the scoped property may still be in scope once the server \
             applies its defaults, so the check only compares what the body sets",
        );
    }

    #[test]
    fn scope_narrows_the_leak_set_to_the_slice_the_op_owns() {
        let m = matcher_for(
            "x:MemoryLookupKey",
            vec![
                json!({ "id": "k1", "namespace": "traps", "key": "a" }),
                json!({ "id": "k2", "namespace": "other", "key": "b" }),
            ],
        );
        let outcome = UpsertOutcome {
            created: 0,
            updated: 0,
            matched_ids: HashSet::new(),
            scope: Some(vec![("namespace".to_string(), json!("traps"))]),
        };
        assert_eq!(
            leaked_ids(&m, "x:MemoryLookupKey", &outcome),
            vec!["k1".to_string()],
            "an object outside the scope is not the operation's to delete"
        );
    }

    #[test]
    fn scope_reference_may_not_point_at_the_op_that_declares_it() {
        let s = account_schema();
        let raw = vec![RawOp::Reconcile {
            object: "Account".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!({ "domainId": "#self" })),
            value: {
                let mut m = Map::new();
                m.insert("self".into(), json!({ "name": "a", "domainId": "#self" }));
                m
            },
        }];
        let plan = Plan::resolve(&s, raw).expect("plan resolves structurally");
        let err = match validate_plan_references(&s, &plan) {
            Ok(_) => panic!("expected a self-referencing scope to be rejected"),
            Err(e) => e,
        };
        assert!(
            format!("{err}").contains("#self"),
            "a scope is resolved before the operation creates anything, so dry-run must \
             reject what the real run cannot resolve: {err}"
        );
    }

    #[test]
    fn scope_with_a_null_value_is_rejected() {
        let s = domain_schema();
        let raw = vec![RawOp::Reconcile {
            object: "Domain".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!({ "description": Value::Null })),
            value: Map::new(),
        }];
        let err = match Plan::resolve(&s, raw) {
            Ok(_) => panic!("expected a null scope value to be rejected"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("null `scope` value") && msg.contains("widen"),
            "a null scope value matches every object that omits the property, which would \
             widen the operation rather than narrow it: {msg}"
        );
    }

    #[test]
    fn scope_on_an_unknown_property_is_rejected() {
        let s = domain_schema();
        let raw = vec![RawOp::Reconcile {
            object: "Domain".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!({ "nameContains": "spam-" })),
            value: Map::new(),
        }];
        let err = match Plan::resolve(&s, raw) {
            Ok(_) => panic!("expected an unknown scope property to be rejected"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("not a property") && msg.contains("Contains"),
            "an operator-form filter key silently matches nothing, so it must be rejected: {msg}"
        );
    }

    #[test]
    fn scope_accepts_at_type_and_declared_properties() {
        let s = mta_route_multi_schema_with_fields();
        let raw = vec![RawOp::Reconcile {
            object: "MtaRoute".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!({ "@type": "Local", "name": "a" })),
            value: Map::new(),
        }];
        Plan::resolve(&s, raw)
            .expect("`@type` and a property declared by any variant are both valid filter keys");
    }

    #[test]
    fn value_match_rejects_an_ambiguous_candidate_set() {
        let s = domain_schema();
        let m = matcher_with(vec![
            json!({ "id": "srv1", "name": "a.com", "description": Value::Null }),
            json!({ "id": "srv2", "name": "a.com", "description": Value::Null }),
        ]);
        let body = json!({ "name": "a.com" }).as_object().unwrap().clone();
        let err = match find_match(
            &m,
            &s,
            "x:Domain",
            &body,
            &MatchKey::Value,
            None,
            &HashMap::new(),
        ) {
            Ok(_) => panic!("expected an ambiguous value match to be rejected"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("ambiguous match"),
            "picking the first of several identical candidates would silently destroy the rest \
             under reconcile: {msg}"
        );
    }

    #[test]
    fn props_match_treats_an_absent_property_as_null() {
        let s = account_schema();
        let m = matcher_for(
            "x:Account",
            vec![json!({ "id": "srv1", "name": "a", "domainId": Value::Null })],
        );
        let body = json!({ "name": "a", "domainId": Value::Null })
            .as_object()
            .unwrap()
            .clone();
        let key = MatchKey::Props(vec!["name".into(), "domainId".into()]);
        let got = find_match(&m, &s, "x:Account", &body, &key, None, &HashMap::new()).unwrap();
        assert_eq!(
            got.as_deref(),
            Some("srv1"),
            "both match branches must agree on null handling"
        );
    }

    #[test]
    fn scope_is_rejected_on_create_update_and_destroy() {
        for op in ["create", "update", "destroy"] {
            let input = format!(
                "{{\"@type\":\"{op}\",\"object\":\"Domain\",\"scope\":{{\"name\":\"a\"}},\
                 \"value\":{{\"d1\":{{\"name\":\"a\"}}}}}}\n"
            );
            let raw = parse_ndjson_plan(&input).expect("the op parses");
            let err = match Plan::resolve(&domain_schema(), raw) {
                Ok(_) => panic!("expected a scope on `{op}` to be rejected"),
                Err(e) => e,
            };
            assert!(
                format!("{err}").contains("only upsert and reconcile match"),
                "a scope on `{op}` must not be silently ignored"
            );
        }
    }

    #[test]
    fn malformed_match_on_reports_the_offending_value() {
        for (input, expected) in [
            (
                "{\"@type\":\"upsert\",\"object\":\"Domain\",\"matchOn\":5,\"value\":{}}",
                "integer",
            ),
            (
                "{\"@type\":\"upsert\",\"object\":\"Domain\",\"matchOn\":[\"a\",2],\"value\":{}}",
                "expected a string",
            ),
            (
                "{\"@type\":\"upsert\",\"object\":\"Domain\",\"matchOn\":{},\"value\":{}}",
                "a list of property names",
            ),
        ] {
            let err = match parse_ndjson_plan(input) {
                Ok(_) => panic!("expected a malformed matchOn to be rejected: {input}"),
                Err(e) => format!("{e}"),
            };
            assert!(
                err.contains(expected) && !err.contains("untagged"),
                "expected a message naming the offending value, got: {err}"
            );
        }
    }

    #[test]
    fn wildcard_match_on_is_accepted_on_an_upsert() {
        let s = domain_schema();
        let raw = vec![RawOp::Upsert {
            object: "Domain".into(),
            match_on: Some(MatchOn::Wildcard("*".into())),
            scope: None,
            value: {
                let mut m = Map::new();
                m.insert("d1".into(), json!({ "name": "a.com" }));
                m
            },
        }];
        Plan::resolve(&s, raw).expect("an upsert may opt into value matching explicitly");
    }

    #[test]
    fn reconcile_with_wildcard_match_on_opts_into_value_matching() {
        let mut s = Schema::default();
        s.objects.insert("x:Tracer".into(), obj_type());
        let raw = vec![RawOp::Reconcile {
            object: "Tracer".into(),
            match_on: Some(MatchOn::Wildcard("*".into())),
            scope: None,
            value: {
                let mut m = Map::new();
                m.insert("t1".into(), json!({ "name": "x" }));
                m
            },
        }];
        let plan = Plan::resolve(&s, raw)
            .expect("an explicit wildcard matchOn opts into value matching on a keyless type");
        assert_eq!(plan.reconciles, 1);
    }

    #[test]
    fn wildcard_match_on_resolves_to_value_matching() {
        let s = domain_schema();
        assert!(
            matches!(
                resolve_match_key(&s, "x:Domain", Some(&MatchOn::Wildcard("*".into()))),
                MatchKey::Value
            ),
            "an explicit wildcard must override the label property"
        );
    }

    #[test]
    fn non_wildcard_match_on_string_is_rejected() {
        let s = domain_schema();
        let raw = vec![RawOp::Reconcile {
            object: "Domain".into(),
            match_on: Some(MatchOn::Wildcard("all".into())),
            scope: None,
            value: Map::new(),
        }];
        let err = match Plan::resolve(&s, raw) {
            Ok(_) => panic!("expected an error for a matchOn string other than `*`"),
            Err(e) => e,
        };
        assert!(format!("{err}").contains("invalid `matchOn` value"));
    }

    #[test]
    fn parses_match_on_wildcard_and_scope() {
        let input = "{\"@type\":\"reconcile\",\"object\":\"MtaRoute\",\"matchOn\":\"*\",\
                     \"scope\":{\"@type\":\"Local\"},\"value\":{}}\n";
        let ops = parse_ndjson_plan(input).unwrap();
        match &ops[0] {
            RawOp::Reconcile {
                match_on, scope, ..
            } => {
                assert!(matches!(match_on, Some(MatchOn::Wildcard(w)) if w == "*"));
                assert_eq!(
                    scope.as_ref().and_then(|s| s.get("@type")),
                    Some(&json!("Local"))
                );
            }
            other => panic!("expected reconcile, got {other:?}"),
        }
    }

    #[test]
    fn scope_restricts_which_candidates_an_upsert_may_match() {
        let s = account_schema();
        let m = matcher_for(
            "x:Account",
            vec![
                json!({ "id": "in", "name": "shared", "domainId": "dom-1" }),
                json!({ "id": "out", "name": "shared", "domainId": "dom-2" }),
            ],
        );
        let body = json!({ "name": "shared", "domainId": "dom-1" })
            .as_object()
            .unwrap()
            .clone();
        let key = MatchKey::Props(vec!["name".into()]);

        let err = find_match(&m, &s, "x:Account", &body, &key, None, &HashMap::new())
            .expect_err("without a scope both domains are candidates and the match is ambiguous");
        assert!(format!("{err}").contains("ambiguous"));

        let scope = vec![("domainId".to_string(), json!("dom-1"))];
        let got = find_match(
            &m,
            &s,
            "x:Account",
            &body,
            &key,
            Some(&scope),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            got.as_deref(),
            Some("in"),
            "a scope keeps an operation from reaching across into a slice it does not own"
        );
    }

    #[test]
    fn scope_must_be_an_object() {
        let s = domain_schema();
        let raw = vec![RawOp::Reconcile {
            object: "Domain".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!("Local")),
            value: Map::new(),
        }];
        let err = match Plan::resolve(&s, raw) {
            Ok(_) => panic!("expected an error for a non-object scope"),
            Err(e) => e,
        };
        assert!(format!("{err}").contains("not a JSON object"));
    }

    #[test]
    fn empty_scope_is_treated_as_absent() {
        let s = domain_schema();
        let raw = vec![RawOp::Reconcile {
            object: "Domain".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!({})),
            value: Map::new(),
        }];
        let plan = Plan::resolve(&s, raw).expect("an empty scope resolves");
        assert!(
            matches!(&plan.ops[0], ResolvedOp::Reconcile { scope: None, .. }),
            "an empty scope must not narrow the operation"
        );
    }

    #[test]
    fn scope_resolves_a_client_reference() {
        let s = account_schema();
        let mut created = HashMap::new();
        created.insert("dom-a".to_string(), "srv-dom".to_string());
        let filter = json!({ "domainId": "#dom-a" }).as_object().unwrap().clone();
        let resolved = resolve_scope(&s, "x:Account", &filter, &created)
            .expect("a declared reference resolves");
        assert_eq!(
            resolved,
            vec![("domainId".to_string(), json!("srv-dom"))],
            "a reference in a scope resolves to the server id before comparison"
        );
    }

    #[test]
    fn scope_keeps_a_literal_hash_in_a_non_reference_field() {
        let s = account_schema();
        let filter = json!({ "name": "#literal" }).as_object().unwrap().clone();
        let resolved = resolve_scope(&s, "x:Account", &filter, &HashMap::new())
            .expect("a `#` in a String field is a literal, not a reference");
        assert_eq!(resolved, vec![("name".to_string(), json!("#literal"))]);
    }

    #[test]
    fn scope_unresolved_reference_is_rejected_upfront() {
        let s = account_schema();
        let raw = vec![RawOp::Reconcile {
            object: "Account".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!({ "domainId": "#ghost" })),
            value: {
                let mut m = Map::new();
                m.insert("acc".into(), json!({ "name": "a" }));
                m
            },
        }];
        let plan = Plan::resolve(&s, raw).expect("plan resolves structurally");
        let err = match validate_plan_references(&s, &plan) {
            Ok(_) => panic!("expected an unresolved scope reference to be rejected"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("`scope`") && msg.contains("#ghost"),
            "expected an unresolved scope reference error: {msg}"
        );
    }

    #[test]
    fn leaked_ids_multi_variant_leaks_every_variant_without_a_filter() {
        let m = matcher_for(
            "x:MtaRoute",
            vec![
                json!({ "id": "l1", "@type": "Local", "name": "a" }),
                json!({ "id": "m1", "@type": "Mx", "name": "b" }),
                json!({ "id": "x1", "name": "no-type" }),
            ],
        );
        let outcome = UpsertOutcome {
            created: 0,
            updated: 1,
            matched_ids: ["l1".to_string()].into_iter().collect(),
            scope: None,
        };
        let mut leaked = leaked_ids(&m, "x:MtaRoute", &outcome);
        leaked.sort();
        assert_eq!(
            leaked,
            vec!["m1".to_string(), "x1".to_string()],
            "an unfiltered reconcile converges the whole type, not just the variants it names"
        );
    }

    #[test]
    fn leaked_ids_multi_variant_empty_plan_deletes_everything() {
        let m = matcher_for(
            "x:MtaRoute",
            vec![
                json!({ "id": "l1", "@type": "Local", "name": "a" }),
                json!({ "id": "m1", "@type": "Mx", "name": "b" }),
            ],
        );
        let outcome = UpsertOutcome {
            created: 0,
            updated: 0,
            matched_ids: HashSet::new(),
            scope: None,
        };
        let mut leaked = leaked_ids(&m, "x:MtaRoute", &outcome);
        leaked.sort();
        assert_eq!(
            leaked,
            vec!["l1".to_string(), "m1".to_string()],
            "an empty-value reconcile deletes the whole type regardless of variant count"
        );
    }

    #[test]
    fn leaked_ids_scope_restricts_deletion_to_one_variant() {
        let m = matcher_for(
            "x:MtaRoute",
            vec![
                json!({ "id": "l1", "@type": "Local", "name": "a" }),
                json!({ "id": "l2", "@type": "Local", "name": "b" }),
                json!({ "id": "m1", "@type": "Mx", "name": "c" }),
                json!({ "id": "x1", "name": "no-type" }),
            ],
        );
        let outcome = UpsertOutcome {
            created: 0,
            updated: 1,
            matched_ids: ["l1".to_string()].into_iter().collect(),
            scope: Some(vec![("@type".to_string(), json!("Local"))]),
        };
        let leaked = leaked_ids(&m, "x:MtaRoute", &outcome);
        assert_eq!(
            leaked,
            vec!["l2".to_string()],
            "only unmatched candidates inside the operation's scope are destroyed"
        );
    }

    #[test]
    fn leaked_ids_scope_requires_every_property_to_match() {
        let m = matcher_for(
            "x:MemoryLookupKey",
            vec![
                json!({ "id": "k1", "namespace": "traps", "isGlobPattern": true }),
                json!({ "id": "k2", "namespace": "traps", "isGlobPattern": false }),
                json!({ "id": "k3", "namespace": "other", "isGlobPattern": true }),
            ],
        );
        let outcome = UpsertOutcome {
            created: 0,
            updated: 0,
            matched_ids: HashSet::new(),
            scope: Some(vec![
                ("namespace".to_string(), json!("traps")),
                ("isGlobPattern".to_string(), json!(true)),
            ]),
        };
        let leaked = leaked_ids(&m, "x:MemoryLookupKey", &outcome);
        assert_eq!(
            leaked,
            vec!["k1".to_string()],
            "a compound scope is an AND over every property"
        );
    }

    #[test]
    fn validate_plan_references_ignores_literal_hash_in_text_prop() {
        let s = account_schema();
        let raw = vec![RawOp::Upsert {
            object: "Account".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: None,
            value: {
                let mut m = Map::new();
                m.insert("acc".into(), json!({ "@type": "User", "name": "#literal" }));
                m
            },
        }];
        let plan = Plan::resolve(&s, raw).expect("plan resolves structurally");
        validate_plan_references(&s, &plan)
            .expect("a `#` in a non-ObjectId matchOn field is a literal, not a reference");
    }

    #[test]
    fn record_created_makes_a_later_match_succeed() {
        let s = domain_schema();
        let mut m = matcher_with(vec![]);
        let key = resolve_match_key(&s, "x:Domain", None);
        let body = json!({ "name": "a.com" }).as_object().unwrap().clone();

        assert_eq!(
            find_match(&m, &s, "x:Domain", &body, &key, None, &HashMap::new()).unwrap(),
            None,
            "nothing exists yet"
        );

        m.record_created(
            "x:Domain",
            vec![
                json!({ "id": "srv9", "name": "a.com" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ],
        );

        assert_eq!(
            find_match(&m, &s, "x:Domain", &body, &key, None, &HashMap::new())
                .unwrap()
                .as_deref(),
            Some("srv9"),
            "a second upsert of the same key in one plan must match what the first one created"
        );
    }

    #[test]
    fn record_created_ignores_types_that_are_not_cached() {
        let mut m = Matcher::new();
        m.record_created(
            "x:Domain",
            vec![
                json!({ "id": "srv9", "name": "a.com" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ],
        );
        assert!(
            !m.objects.contains_key("x:Domain"),
            "recording must not fabricate a cache entry, or `ensure` would skip the full fetch"
        );
        assert!(m.objects_for("x:Domain").is_empty());
    }

    #[test]
    fn record_updated_patches_the_cached_object() {
        let s = domain_schema();
        let mut m = matcher_with(vec![json!({ "id": "srv1", "name": "old.com" })]);
        let patch = json!({ "name": "new.com" }).as_object().unwrap().clone();
        m.record_updated("x:Domain", "srv1", &patch);

        let key = resolve_match_key(&s, "x:Domain", None);
        let body = json!({ "name": "new.com" }).as_object().unwrap().clone();
        assert_eq!(
            find_match(&m, &s, "x:Domain", &body, &key, None, &HashMap::new())
                .unwrap()
                .as_deref(),
            Some("srv1"),
            "a later match key must see the value an earlier op wrote"
        );
    }

    #[test]
    fn record_updated_clears_nulls_and_skips_pointer_paths() {
        let mut m = matcher_with(vec![
            json!({ "id": "srv1", "name": "a.com", "description": "d" }),
        ]);
        let patch = json!({ "description": null, "settings/timeout": 30 })
            .as_object()
            .unwrap()
            .clone();
        m.record_updated("x:Domain", "srv1", &patch);

        let cached = &m.objects_for("x:Domain")[0];
        assert!(
            !cached.contains_key("description"),
            "null clears the property"
        );
        assert!(
            !cached.contains_key("settings/timeout"),
            "JSON-pointer patch keys are not top-level properties"
        );
        assert_eq!(cached.get("name"), Some(&json!("a.com")));
    }

    #[test]
    fn record_updated_ignores_unknown_ids_and_uncached_types() {
        let patch = json!({ "name": "b.com" }).as_object().unwrap().clone();
        let mut m = matcher_with(vec![json!({ "id": "srv1", "name": "a.com" })]);
        m.record_updated("x:Domain", "nope", &patch);
        assert_eq!(
            m.objects_for("x:Domain")[0].get("name"),
            Some(&json!("a.com"))
        );

        let mut empty = Matcher::new();
        empty.record_updated("x:Domain", "srv1", &patch);
        assert!(!empty.objects.contains_key("x:Domain"));
    }

    #[test]
    fn invalidate_drops_the_cached_snapshot() {
        let mut m = matcher_with(vec![json!({ "id": "srv1", "name": "a.com" })]);
        m.invalidate("x:Domain");
        assert!(
            !m.objects.contains_key("x:Domain"),
            "the next `ensure` must refetch after a destroy"
        );
    }

    #[test]
    fn duplicate_match_keys_in_one_op_are_rejected() {
        let s = domain_schema();
        let key = resolve_match_key(
            &s,
            "x:Domain",
            Some(&MatchOn::Props(vec!["name".to_string()])),
        );
        let mut value = Map::new();
        value.insert("a".into(), json!({ "name": "dup.com" }));
        value.insert("b".into(), json!({ "name": "dup.com" }));

        let err = reject_duplicate_match_keys(
            &s,
            "x:Domain",
            &entries_of(&value, None),
            &key,
            &HashMap::new(),
            2,
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("operation #3"), "message names the op: {msg}");
        assert!(
            msg.contains("`a`") && msg.contains("`b`"),
            "message names both entries: {msg}"
        );
        assert!(msg.contains("same match key"), "{msg}");
    }

    #[test]
    fn distinct_match_keys_in_one_op_are_allowed() {
        let s = domain_schema();
        let key = resolve_match_key(&s, "x:Domain", None);
        let mut value = Map::new();
        value.insert("a".into(), json!({ "name": "a.com" }));
        value.insert("b".into(), json!({ "name": "b.com" }));
        reject_duplicate_match_keys(
            &s,
            "x:Domain",
            &entries_of(&value, None),
            &key,
            &HashMap::new(),
            0,
        )
        .expect("distinct keys are fine");
    }

    #[test]
    fn duplicate_check_needs_field_metadata_to_compare_values() {
        let mut bare = Schema::default();
        bare.objects.insert("x:Tracer".into(), obj_type());
        let key = resolve_match_key(&bare, "x:Tracer", None);
        let mut value = Map::new();
        value.insert("a".into(), json!({ "name": "same" }));
        value.insert("b".into(), json!({ "name": "same" }));
        reject_duplicate_match_keys(
            &bare,
            "x:Tracer",
            &entries_of(&value, None),
            &key,
            &HashMap::new(),
            0,
        )
        .expect("with no declared fields there is nothing to compare; find_match reports it");
    }

    #[test]
    fn duplicate_check_sees_entries_that_only_match_after_inheriting_the_scope() {
        let s = lookup_key_schema();
        let scope = vec![("namespace".to_string(), json!("traps"))];
        let mut value = Map::new();
        value.insert("a".into(), json!({ "key": "foo@*" }));
        value.insert("b".into(), json!({ "key": "foo@*" }));
        let key = MatchKey::Props(vec!["namespace".into(), "key".into()]);
        let err = match reject_duplicate_match_keys(
            &s,
            "x:MemoryLookupKey",
            &entries_of(&value, Some(&scope)),
            &key,
            &HashMap::new(),
            0,
        ) {
            Ok(_) => panic!("expected two entries sharing an inherited match key to be rejected"),
            Err(e) => e,
        };
        assert!(
            format!("{err}").contains("same match key"),
            "both entries inherit `namespace` from the scope, so they collide; checking the \
             un-inherited bodies would skip them and create a duplicate: {err}"
        );
    }

    #[test]
    fn duplicate_check_sees_a_stated_and_an_inherited_scope_value_as_equal() {
        let s = lookup_key_schema();
        let scope = vec![("namespace".to_string(), json!("traps"))];
        let mut value = Map::new();
        value.insert("a".into(), json!({ "key": "foo@*" }));
        value.insert("b".into(), json!({ "namespace": "traps", "key": "foo@*" }));
        let err = reject_duplicate_match_keys(
            &s,
            "x:MemoryLookupKey",
            &entries_of(&value, Some(&scope)),
            &MatchKey::Value,
            &HashMap::new(),
            0,
        )
        .expect_err("spelling the scope out must not hide a duplicate under value matching");
        assert!(format!("{err}").contains("identical values"), "{err}");
    }

    #[test]
    fn scope_on_a_server_set_property_is_rejected() {
        let s = server_set_scope_schema();
        let raw = vec![RawOp::Upsert {
            object: "Cert".into(),
            match_on: Some(MatchOn::Props(vec!["name".into()])),
            scope: Some(json!({ "fingerprint": "abc" })),
            value: {
                let mut m = Map::new();
                m.insert("c".into(), json!({ "name": "a" }));
                m
            },
        }];
        let err = match Plan::resolve(&s, raw) {
            Ok(_) => panic!("expected a scope on a server-derived property to be rejected"),
            Err(e) => e,
        };
        assert!(
            format!("{err}").contains("which the server derives"),
            "an entry cannot be created into a ServerSet scope, so it would be recreated on \
             every apply: {err}"
        );
    }

    #[test]
    fn duplicate_check_separates_variants_of_a_multi_variant_type() {
        let s = mta_route_multi_schema();
        let key = resolve_match_key(
            &s,
            "x:MtaRoute",
            Some(&MatchOn::Props(vec!["name".to_string()])),
        );
        let mut value = Map::new();
        value.insert("a".into(), json!({ "@type": "Local", "name": "same" }));
        value.insert("b".into(), json!({ "@type": "Mx", "name": "same" }));
        reject_duplicate_match_keys(
            &s,
            "x:MtaRoute",
            &entries_of(&value, None),
            &key,
            &HashMap::new(),
            0,
        )
        .expect("the same name under two variants is not the same object");

        value.insert("c".into(), json!({ "@type": "Local", "name": "same" }));
        reject_duplicate_match_keys(
            &s,
            "x:MtaRoute",
            &entries_of(&value, None),
            &key,
            &HashMap::new(),
            0,
        )
        .expect_err("two Local routes with the same name collide");
    }

    #[test]
    fn duplicate_check_resolves_references_before_comparing() {
        let s = account_schema();
        let key = resolve_match_key(
            &s,
            "x:Account",
            Some(&MatchOn::Props(vec![
                "name".to_string(),
                "domainId".to_string(),
            ])),
        );
        let mut created = HashMap::new();
        created.insert("dom-a".to_string(), "srv-dom".to_string());

        let mut value = Map::new();
        value.insert(
            "acc1".into(),
            json!({ "name": "jane", "domainId": "#dom-a" }),
        );
        value.insert(
            "acc2".into(),
            json!({ "name": "jane", "domainId": "srv-dom" }),
        );
        let err = reject_duplicate_match_keys(
            &s,
            "x:Account",
            &entries_of(&value, None),
            &key,
            &created,
            0,
        )
        .unwrap_err();
        assert!(
            format!("{err}").contains("same match key"),
            "a `#ref` and the server id it resolves to are the same key"
        );
    }

    #[test]
    fn duplicate_check_defers_entries_it_cannot_resolve_yet() {
        let s = account_schema();
        let key = resolve_match_key(
            &s,
            "x:Account",
            Some(&MatchOn::Props(vec!["domainId".to_string()])),
        );
        let mut value = Map::new();
        value.insert(
            "acc1".into(),
            json!({ "name": "jane", "domainId": "#later" }),
        );
        value.insert("acc2".into(), json!({ "name": "john" }));
        reject_duplicate_match_keys(
            &s,
            "x:Account",
            &entries_of(&value, None),
            &key,
            &HashMap::new(),
            0,
        )
        .expect("unresolved references and missing match properties stay `find_match`'s to report");
    }
}
