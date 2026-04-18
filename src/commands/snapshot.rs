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
    FieldType, Fields, MapValueType, ObjectSchema, ObjectType, ObjectVariant, ScalarType, Schema,
    StringFormat,
};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
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
    sink.write_all(b"[")?;
    let mut first = true;

    if !args.no_destroys {
        for shard in plan.iter_non_singletons() {
            emit_destroy(&mut sink, &mut first, shard)?;
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

    for shard in plan.iter_non_singletons() {
        emit_create(&mut sink, &mut first, &snap_ctx, shard, &mut reporter)?;
    }

    for name in plan.singletons() {
        emit_singleton_update(&mut sink, &mut first, &snap_ctx, name, &mut reporter)?;
    }

    sink.write_all(b"]\n")?;
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

#[derive(Clone, PartialEq, Eq, Hash)]
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
}

impl Plan {
    fn iter_non_singletons(&self) -> impl Iterator<Item = &Shard> {
        self.shards.iter().filter(|s| !s.is_singleton)
    }
    fn singletons(&self) -> &[String] {
        &self.singletons
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

    topologically_sort(schema, &mut shards)?;
    Ok(Plan { shards, singletons })
}

fn topologically_sort(schema: &Schema, shards: &mut Vec<Shard>) -> CliResult<()> {
    let index: HashMap<ShardKey, usize> = shards
        .iter()
        .enumerate()
        .map(|(i, s)| (s.key.clone(), i))
        .collect();

    let mut deps: Vec<HashSet<usize>> = vec![HashSet::new(); shards.len()];
    for (i, shard) in shards.iter().enumerate() {
        let fields = shard_fields(schema, &shard.key);
        if let Some(f) = fields {
            let mut refs: Vec<ShardKey> = Vec::new();
            let mut visited: HashSet<String> = HashSet::new();
            collect_shard_refs(schema, f, &mut refs, &mut visited);
            for r in refs {
                if let Some(&j) = index.get(&r)
                    && j != i
                {
                    deps[i].insert(j);
                }
            }
        }
    }

    let n = shards.len();
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

    if order.len() != n {
        let cycle: Vec<String> = (0..n)
            .filter(|&j| !deps[j].is_empty())
            .map(|j| {
                let k = &shards[j].key;
                match &k.variant {
                    Some(v) => format!("{}/{}", resolve::display_name(&k.canonical), v),
                    None => resolve::display_name(&k.canonical).to_string(),
                }
            })
            .collect();
        return Err(CliError::msg(format!(
            "cannot snapshot: cyclic dependency between the selected types ({})",
            cycle.join(", ")
        )));
    }

    let mut taken: Vec<Option<Shard>> = shards.drain(..).map(Some).collect();
    for i in order {
        if let Some(s) = taken[i].take() {
            shards.push(s);
        }
    }
    Ok(())
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
                return Err(CliError::msg(format!(
                    "{} references {} but {} is not in the snapshot selection; \
                     add it or use --allow-unresolved {}",
                    resolve::display_name(canonical),
                    resolve::display_name(&r.canonical),
                    resolve::display_name(&r.canonical),
                    resolve::display_name(&r.canonical),
                )));
            }
        }
    }
    Ok(())
}

fn emit_destroy<W: Write>(sink: &mut W, first: &mut bool, shard: &Shard) -> CliResult<()> {
    if !*first {
        sink.write_all(b",\n")?;
    }
    *first = false;
    sink.write_all(b"{\"@type\":\"destroy\",\"object\":\"")?;
    sink.write_all(resolve::display_name(&shard.key.canonical).as_bytes())?;
    sink.write_all(b"\"")?;
    if let Some(variant) = &shard.key.variant {
        sink.write_all(b",\"value\":{\"@type\":\"")?;
        sink.write_all(variant.as_bytes())?;
        sink.write_all(b"\"}")?;
    }
    sink.write_all(b"}")?;
    Ok(())
}

fn emit_create<W: Write>(
    sink: &mut W,
    first: &mut bool,
    cx: &Ctx<'_>,
    shard: &Shard,
    reporter: &mut Reporter,
) -> CliResult<()> {
    let jmap = cx.jmap;
    let schema = cx.schema;
    let allow = cx.allow;
    let include_secrets = cx.include_secrets;
    let limit = cx.limit;
    if !*first {
        sink.write_all(b",\n")?;
    }
    *first = false;
    sink.write_all(b"{\"@type\":\"create\",\"object\":\"")?;
    sink.write_all(resolve::display_name(&shard.key.canonical).as_bytes())?;
    sink.write_all(b"\",\"value\":{")?;

    let fields = shard_fields(schema, &shard.key).ok_or_else(|| {
        CliError::UnexpectedResponse(format!("no schema available for {}", shard.key.canonical))
    })?;
    let query_method = format!("{}/query", shard.key.canonical);
    let get_method = format!("{}/get", shard.key.canonical);
    let filter = shard_filter(&shard.key);

    let mut anchor: Option<String> = None;
    let mut first_entry = true;
    let mut total = 0usize;
    reporter.shard_start(&shard.key);

    loop {
        let mut q_args = Map::new();
        q_args.insert("filter".into(), filter.clone());
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
            let Some(obj) = item.as_object() else {
                continue;
            };
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

            if !first_entry {
                sink.write_all(b",")?;
            }
            first_entry = false;
            sink.write_all(b"\"")?;
            write_client_id(sink, &shard.key.canonical, server_id)?;
            sink.write_all(b"\":")?;
            serde_json::to_writer(&mut *sink, &Value::Object(out_obj))?;
        }
        total += returned;

        if returned < limit {
            break;
        }
        anchor = last_id;
        if anchor.is_none() {
            break;
        }
    }

    sink.write_all(b"}}")?;
    reporter.shard_done(&shard.key, total);
    Ok(())
}

fn shard_filter(key: &ShardKey) -> Value {
    match &key.variant {
        Some(v) => json!({ "@type": v }),
        None => json!({}),
    }
}

fn emit_singleton_update<W: Write>(
    sink: &mut W,
    first: &mut bool,
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

    if !*first {
        sink.write_all(b",\n")?;
    }
    *first = false;
    sink.write_all(b"{\"@type\":\"update\",\"object\":\"")?;
    sink.write_all(resolve::display_name(canonical).as_bytes())?;
    sink.write_all(b"\",\"value\":")?;
    serde_json::to_writer(&mut *sink, &Value::Object(transformed))?;
    sink.write_all(b"}")?;
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
            schema.fields.get(v.schema_name.as_ref()?)?
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
    fn shard_start(&mut self, key: &ShardKey) {
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        match &key.variant {
            Some(v) => {
                let _ = writeln!(
                    err,
                    "  fetching {} ({})...",
                    resolve::display_name(&key.canonical),
                    v
                );
            }
            None => {
                let _ = writeln!(
                    err,
                    "  fetching {}...",
                    resolve::display_name(&key.canonical)
                );
            }
        }
    }
    fn shard_done(&mut self, _key: &ShardKey, count: usize) {
        if self.quiet {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "    -> {count}");
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

#[allow(dead_code)]
fn _keep_variant_symbol(_: &ObjectVariant) {}

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
}
