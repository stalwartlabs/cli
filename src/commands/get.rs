/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::context::Context;
use crate::app::error::{CliError, CliResult};
use crate::cli::GetArgs;
use crate::jmap::Jmap;
use crate::render::Ansi;
use crate::render::object::{RenderCtx, render_top};
use crate::schema::resolve;
use crate::schema::{ObjectSchema, ObjectType};
use crate::util::display_cache::DisplayCache;
use serde_json::{Value, json};
use std::io::Write;

pub fn run(ctx: &Context, args: &GetArgs) -> CliResult<()> {
    resolve::forbid_slash_form(&args.object)?;
    let canonical = resolve::require_object(&ctx.schema, &args.object)?;
    let is_singleton = matches!(
        ctx.schema.objects.get(canonical),
        Some(ObjectType::Singleton { .. })
    );

    let id = resolve_id(canonical, is_singleton, args.id.as_deref())?;

    let properties = match &args.fields {
        Some(fs) => Some(canonicalise_fields(&ctx.schema, canonical, fs)?),
        None => None,
    };

    let jmap = Jmap::new(&ctx.client, &ctx.session.api_path);
    let method = format!("{}/get", canonical);
    let request_args = json!({
        "ids": [id],
        "properties": properties,
    });

    let result = jmap.call(&method, request_args)?;
    let list = result
        .get("list")
        .and_then(Value::as_array)
        .ok_or_else(|| CliError::UnexpectedResponse("missing 'list' in get response".into()))?;

    if list.is_empty() {
        return Err(CliError::msg(format!(
            "{} {} not found",
            resolve::display_name(canonical),
            id
        )));
    }

    let obj_value = &list[0];
    let Some(obj) = obj_value.as_object() else {
        return Err(CliError::UnexpectedResponse(
            "object is not a JSON object".into(),
        ));
    };

    if args.json {
        let compact = serde_json::to_string(obj_value)?;
        writeln!(std::io::stdout(), "{compact}")?;
        return Ok(());
    }

    let mut cache = DisplayCache::new();
    let fields_for_cache = fields_for_object(&ctx.schema, canonical, obj);
    if let Some(f) = fields_for_cache {
        cache.populate_from_objects(&jmap, &ctx.schema, f, &[obj])?;
    }

    let ansi = Ansi::new(ctx.config.color);
    let render_ctx = RenderCtx {
        schema: &ctx.schema,
        cache: &cache,
        ansi,
    };
    let mut out = String::new();
    render_top(&mut out, &render_ctx, canonical, obj);
    std::io::stdout().write_all(out.as_bytes())?;
    Ok(())
}

fn resolve_id(object: &str, is_singleton: bool, id: Option<&str>) -> CliResult<String> {
    if is_singleton {
        match id {
            None | Some("singleton") => Ok("singleton".to_string()),
            Some(_) => Err(CliError::BadSingletonId),
        }
    } else {
        match id {
            Some(v) if !v.is_empty() => Ok(v.to_string()),
            _ => Err(CliError::IdRequired(
                resolve::display_name(object).to_string(),
            )),
        }
    }
}

fn canonicalise_fields(
    schema: &crate::schema::Schema,
    canonical: &str,
    fs: &[String],
) -> CliResult<Vec<String>> {
    let obj_schema = schema
        .schemas
        .get(canonical)
        .ok_or_else(|| CliError::UnexpectedResponse(format!("schema missing for {canonical}")))?;

    let allowed: Vec<&str> = match obj_schema {
        ObjectSchema::Single { schema_name } => schema
            .fields
            .get(schema_name)
            .map(|f| f.properties.keys().map(String::as_str).collect())
            .unwrap_or_default(),
        ObjectSchema::Multiple { variants } => {
            let mut names: Vec<&str> = Vec::new();
            for v in variants {
                if let Some(sn) = &v.schema_name
                    && let Some(f) = schema.fields.get(sn)
                {
                    for k in f.properties.keys() {
                        let s = k.as_str();
                        if !names.contains(&s) {
                            names.push(s);
                        }
                    }
                }
            }
            names
        }
    };

    let mut out = Vec::with_capacity(fs.len());
    for raw in fs {
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("id") {
            out.push("id".to_string());
            continue;
        }
        if trimmed.eq_ignore_ascii_case("@type") {
            out.push("@type".to_string());
            continue;
        }
        let matched = allowed
            .iter()
            .find(|k| k.eq_ignore_ascii_case(trimmed))
            .copied();
        match matched {
            Some(canon) => out.push(canon.to_string()),
            None => return Err(CliError::UnknownField(trimmed.to_string())),
        }
    }
    Ok(out)
}

fn fields_for_object<'a>(
    schema: &'a crate::schema::Schema,
    object_name: &str,
    obj: &serde_json::Map<String, Value>,
) -> Option<&'a crate::schema::Fields> {
    match schema.schemas.get(object_name)? {
        ObjectSchema::Single { schema_name } => schema.fields.get(schema_name),
        ObjectSchema::Multiple { variants } => {
            let at_type = obj.get("@type").and_then(Value::as_str)?;
            let v = variants.iter().find(|v| v.name == at_type)?;
            let sn = v.schema_name.as_ref()?;
            schema.fields.get(sn)
        }
    }
}
