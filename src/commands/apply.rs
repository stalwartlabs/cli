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
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

pub fn run(ctx: &Context, args: &ApplyArgs) -> CliResult<()> {
    let input = read_input(args)?;
    let raw_ops = parse_ndjson_plan(&input)?;

    let plan = Plan::resolve(&ctx.schema, raw_ops)?;

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
        summary: Summary::new(&plan),
    };
    let mut matcher = Matcher::new();

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
                value,
                index,
            } => {
                match execute_upsert(
                    ctx,
                    &jmap,
                    canonical,
                    match_on.as_deref(),
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
            ResolvedOp::Update {
                canonical,
                id,
                value,
                index,
            } => match execute_update(&jmap, canonical, id, value, &mut state.created_ids) {
                Ok(count) => {
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
                    Ok(count) => {
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

#[derive(Deserialize, Debug)]
#[serde(tag = "@type", rename_all = "lowercase")]
enum RawOp {
    Update {
        object: String,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        value: Value,
    },
    Destroy {
        object: String,
        #[serde(default)]
        value: Option<Value>,
    },
    Create {
        object: String,
        #[serde(default)]
        value: Map<String, Value>,
    },
    Upsert {
        object: String,
        #[serde(default, rename = "matchOn")]
        match_on: Option<Vec<String>>,
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
        match_on: Option<Vec<String>>,
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

        for (index, r) in raw.into_iter().enumerate() {
            match r {
                RawOp::Update { object, id, value } => {
                    let canonical = resolve::require_object(schema, &object)?;
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
                RawOp::Destroy { object, value } => {
                    let canonical = resolve::require_object(schema, &object)?;
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
                RawOp::Create { object, value } => {
                    let canonical = resolve::require_object(schema, &object)?;
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
                    if let Some(keys) = &match_on
                        && keys.is_empty()
                    {
                        return Err(CliError::msg(format!(
                            "upsert operation #{} has an empty `matchOn` list",
                            index + 1,
                        )));
                    }

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
    summary: Summary,
}

struct Summary {
    planned_destroys: usize,
    planned_updates: usize,
    planned_creates: usize,
    planned_create_objects: usize,
    planned_upserts: usize,
    planned_upsert_objects: usize,
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
    if ids.is_empty() {
        return Ok(0);
    }

    let batch_size = ctx.session.max_objects_in_set.max(1);
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

fn resolve_match_key(schema: &Schema, canonical: &str, match_on: Option<&[String]>) -> MatchKey {
    if let Some(keys) = match_on {
        return MatchKey::Props(keys.to_vec());
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
        for item in list {
            if let Some(obj) = item.as_object() {
                all.push(obj.clone());
            }
        }
        if returned < limit {
            break;
        }
        match last_id {
            Some(a) => anchor = Some(a),
            None => break,
        }
    }
    Ok(all)
}

fn find_match(
    matcher: &Matcher,
    schema: &Schema,
    canonical: &str,
    body: &Map<String, Value>,
    key: &MatchKey,
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
        if multi {
            o.get("@type").and_then(Value::as_str) == at_type
        } else {
            true
        }
    });

    match key {
        MatchKey::Props(props) => {
            for p in props {
                if !body.contains_key(p) {
                    return Err(CliError::msg(format!(
                        "{}: match property `{}` is missing from the object body",
                        resolve::display_name(canonical),
                        p,
                    )));
                }
            }
            let mut matched: Option<String> = None;
            let mut count = 0usize;
            for cand in candidates {
                if props.iter().all(|p| cand.get(p) == body.get(p)) {
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
            for cand in candidates {
                if props.iter().all(|p| cand.get(*p) == body.get(*p)) {
                    return Ok(cand.get("id").and_then(Value::as_str).map(String::from));
                }
            }
            Ok(None)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_upsert(
    ctx: &Context,
    jmap: &Jmap,
    canonical: &str,
    match_on: Option<&[String]>,
    value: &Map<String, Value>,
    state: &mut State,
    matcher: &mut Matcher,
    op_index: usize,
    reporter: &Reporter,
) -> CliResult<(usize, usize)> {
    matcher.ensure(ctx, jmap, canonical)?;
    let key = resolve_match_key(&ctx.schema, canonical, match_on);
    if matches!(key, MatchKey::Value) && matcher.warned_value_match.insert(canonical.to_string()) {
        reporter.value_match_warning(canonical);
    }

    let mut to_create: Map<String, Value> = Map::new();
    let mut to_update: Vec<(String, Value)> = Vec::new();
    for (client_id, body_val) in value {
        let body = body_val.as_object().ok_or_else(|| {
            CliError::msg(format!(
                "{}: upsert entry `{}` is not a JSON object",
                resolve::display_name(canonical),
                client_id,
            ))
        })?;
        match find_match(matcher, &ctx.schema, canonical, body, &key)? {
            Some(server_id) => {
                state
                    .created_ids
                    .insert(client_id.clone(), server_id.clone());
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
                to_create.insert(client_id.clone(), Value::Object(create_body));
            }
        }
    }

    let mut created = 0usize;
    if !to_create.is_empty() {
        let batch_size = ctx.session.max_objects_in_set.max(1);
        created = execute_create(
            jmap,
            canonical,
            &to_create,
            batch_size,
            &mut state.created_ids,
            op_index,
            reporter,
        )?;
    }

    let mut updated = 0usize;
    for (server_id, body) in &to_update {
        updated += execute_update(jmap, canonical, server_id, body, &mut state.created_ids)?;
    }

    Ok((created, updated))
}

fn execute_update(
    jmap: &Jmap,
    canonical: &str,
    id: &str,
    value: &Value,
    created_ids: &mut HashMap<String, String>,
) -> CliResult<usize> {
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
            set_error::render(&set_err, Ansi::new(false)).replace('\n', " | ")
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
    Ok(updated)
}

fn execute_create(
    jmap: &Jmap,
    canonical: &str,
    value: &Map<String, Value>,
    batch_size: usize,
    created_ids: &mut HashMap<String, String>,
    op_index: usize,
    reporter: &Reporter,
) -> CliResult<usize> {
    let entries: Vec<(&String, &Value)> = value.iter().collect();
    let method = format!("{canonical}/set");
    let total = entries.len();
    let batches = total.div_ceil(batch_size);
    let mut created_count = 0usize;

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
                set_error::render(&set_err, Ansi::new(false)).replace('\n', " | ")
            )));
        }

        if let Some(created) = result.get("created").and_then(Value::as_object) {
            for (user_id, body) in created {
                if let Some(server_id) = body.get("id").and_then(Value::as_str) {
                    created_ids.insert(user_id.clone(), server_id.to_string());
                }
            }
            created_count += created.len();
        }
    }
    Ok(created_count)
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
            "{}Plan:{} {} destroy, {} update, {} create, {} upsert ({} objects)",
            self.ansi.bold(),
            self.ansi.reset(),
            plan.destroys,
            plan.updates,
            plan.creates,
            plan.upserts,
            plan.create_objects + plan.upsert_objects,
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
        _ => "done",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut m = Matcher::new();
        m.objects.insert(
            "x:Domain".into(),
            objs.into_iter()
                .filter_map(|v| v.as_object().cloned())
                .collect(),
        );
        m
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
            } => {
                assert_eq!(object, "Domain");
                assert_eq!(match_on.as_deref(), Some(["name".to_string()].as_slice()));
                assert!(value.contains_key("d1"));
            }
            other => panic!("expected upsert, got {other:?}"),
        }
    }

    #[test]
    fn resolve_match_key_prefers_match_on_then_label_then_value() {
        let s = domain_schema();
        assert!(matches!(
            resolve_match_key(&s, "x:Domain", Some(&["description".to_string()])),
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
        let got = find_match(&m, &s, "x:Domain", &body, &key).unwrap();
        assert_eq!(got.as_deref(), Some("srv2"));

        let body = json!({ "name": "nope.com" }).as_object().unwrap().clone();
        let got = find_match(&m, &s, "x:Domain", &body, &key).unwrap();
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
        let err = find_match(&m, &s, "x:Domain", &body, &key).unwrap_err();
        assert!(format!("{err}").contains("ambiguous"));
    }

    #[test]
    fn find_match_missing_key_property_is_error() {
        let s = domain_schema();
        let m = matcher_with(vec![json!({ "id": "srv1", "name": "a.com" })]);
        let body = json!({ "description": "x" }).as_object().unwrap().clone();
        let key = resolve_match_key(&s, "x:Domain", None);
        let err = find_match(&m, &s, "x:Domain", &body, &key).unwrap_err();
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
        let got = find_match(&m, &s, "x:Domain", &body, &key).unwrap();
        assert_eq!(got.as_deref(), Some("srv2"));

        let changed = json!({ "name": "b.com", "description": "CHANGED" })
            .as_object()
            .unwrap()
            .clone();
        let got = find_match(&m, &s, "x:Domain", &changed, &key).unwrap();
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
}
