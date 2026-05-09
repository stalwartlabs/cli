/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::context::Context;
use crate::app::error::{CliError, CliResult};
use crate::cli::CreateArgs;
use crate::jmap::Jmap;
use crate::jmap::errors::SetError;
use crate::render::Ansi;
use crate::render::object::{RenderCtx, render_top};
use crate::render::set_error;
use crate::schema::resolve;
use crate::schema::{Fields, ObjectSchema};
use crate::util::display_cache::DisplayCache;
use crate::util::input::{self, Mode, Sources};
use serde_json::{Map, Value, json};
use std::io::Write;

pub fn run(ctx: &Context, args: &CreateArgs) -> CliResult<()> {
    let (canonical, variant) = resolve::resolve_create_target(&ctx.schema, &args.object)?;

    let fields = fields_for(&ctx.schema, canonical, variant.as_deref())?;

    let sources = Sources {
        fields: &args.field,
        json: args.json.as_deref(),
        file: args.file.as_deref(),
        stdin: args.stdin,
    };
    let mut input = input::build_input(&sources, &fields, Mode::Create)?;

    if let Some(variant_name) = &variant {
        match input.get("@type") {
            Some(existing) => {
                if existing.as_str() != Some(variant_name.as_str()) {
                    eprintln!("warning: --field `@type` overrides the Object/Variant shorthand");
                }
            }
            None => {
                input.insert("@type".to_string(), Value::String(variant_name.clone()));
            }
        }
    }

    let jmap = Jmap::new(&ctx.client, &ctx.session.api_path);
    let method = format!("{canonical}/set");
    let result = jmap.call(
        &method,
        json!({
            "create": { "n0": Value::Object(input) },
        }),
    )?;

    if let Some(not_created) = result.get("notCreated").and_then(Value::as_object)
        && let Some(err_val) = not_created.get("n0")
    {
        let set_err: SetError = serde_json::from_value(err_val.clone())?;
        let ansi = Ansi::new(ctx.config.color);
        eprintln!("{}", set_error::render(&set_err, ansi));
        return Err(CliError::msg("create failed"));
    }

    let created = result
        .get("created")
        .and_then(Value::as_object)
        .and_then(|m| m.get("n0"))
        .and_then(Value::as_object)
        .ok_or_else(|| CliError::UnexpectedResponse("set response missing created[n0]".into()))?;
    let id = created
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::UnexpectedResponse("created object missing id".into()))?
        .to_string();

    let ansi = Ansi::new(ctx.config.color);
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{}Created{} {} {}{}{}",
        ansi.green(),
        ansi.reset(),
        resolve::display_name(canonical),
        ansi.cyan(),
        id,
        ansi.reset(),
    )?;

    let mut extras: Map<String, Value> = created.clone();
    extras.remove("id");
    if !extras.is_empty() {
        writeln!(stdout)?;
        let mut cache = DisplayCache::new();
        let walk_fields = walk_fields(&ctx.schema, canonical, &extras);
        if let Some(wf) = walk_fields {
            cache.populate_from_objects(&jmap, &ctx.schema, wf, &[&extras])?;
        }
        let render_ctx = RenderCtx {
            schema: &ctx.schema,
            cache: &cache,
            ansi,
        };
        let mut out = String::new();
        render_top(&mut out, &render_ctx, canonical, &extras);
        stdout.write_all(out.as_bytes())?;
    }

    Ok(())
}

fn fields_for(
    schema: &crate::schema::Schema,
    canonical: &str,
    variant: Option<&str>,
) -> CliResult<Fields> {
    let obj_schema = schema
        .schemas
        .get(canonical)
        .ok_or_else(|| CliError::UnexpectedResponse(format!("schema missing for {canonical}")))?;
    match obj_schema {
        ObjectSchema::Single { schema_name } => {
            Ok(schema.fields.get(schema_name).cloned().unwrap_or_default())
        }
        ObjectSchema::Multiple { variants } => {
            if let Some(vn) = variant {
                for v in variants {
                    if v.name.eq_ignore_ascii_case(vn) {
                        return Ok(match &v.schema_name {
                            Some(sn) => schema.fields.get(sn).cloned().unwrap_or_default(),
                            None => Fields::default(),
                        });
                    }
                }
                return Err(CliError::UnexpectedResponse(format!(
                    "variant `{vn}` not found in {canonical}"
                )));
            }

            let mut out = Fields::default();
            for v in variants {
                if let Some(sn) = &v.schema_name
                    && let Some(f) = schema.fields.get(sn)
                {
                    for (k, field) in &f.properties {
                        out.properties
                            .entry(k.clone())
                            .or_insert_with(|| field.clone());
                    }
                }
            }
            Ok(out)
        }
    }
}

fn walk_fields<'a>(
    schema: &'a crate::schema::Schema,
    canonical: &str,
    obj: &Map<String, Value>,
) -> Option<&'a Fields> {
    match schema.schemas.get(canonical)? {
        ObjectSchema::Single { schema_name } => schema.fields.get(schema_name),
        ObjectSchema::Multiple { variants } => {
            let at_type = obj.get("@type").and_then(Value::as_str)?;
            let v = variants.iter().find(|v| v.name == at_type)?;
            let sn = v.schema_name.as_ref()?;
            schema.fields.get(sn)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Field, FieldType, FieldUpdate, ObjectVariant, Schema, StringFormat,
    };
    use std::collections::HashMap;

    fn string_field() -> Field {
        Field {
            description: String::new(),
            typ: FieldType::String {
                format: StringFormat::String,
                min_length: None,
                max_length: None,
                nullable: false,
            },
            update: FieldUpdate::Mutable,
            enterprise: false,
        }
    }

    fn action_schema() -> Schema {
        let mut s = Schema::default();
        s.schemas.insert(
            "x:Action".into(),
            ObjectSchema::Multiple {
                variants: vec![
                    ObjectVariant {
                        name: "ReloadTlsCertificates".into(),
                        label: "Reload TLS".into(),
                        schema_name: None,
                    },
                    ObjectVariant {
                        name: "ClassifySpam".into(),
                        label: "Classify".into(),
                        schema_name: Some("x:SpamClassify".into()),
                    },
                ],
            },
        );
        let mut props: HashMap<String, Field> = HashMap::new();
        props.insert("message".into(), string_field());
        s.fields.insert(
            "x:SpamClassify".into(),
            Fields {
                properties: props,
                defaults: HashMap::new(),
            },
        );
        s
    }

    #[test]
    fn fields_for_payloadless_variant_returns_empty() {
        let schema = action_schema();
        let f = fields_for(&schema, "x:Action", Some("ReloadTlsCertificates")).unwrap();
        assert!(
            f.properties.is_empty(),
            "payloadless variant must not inherit fields from sibling variants"
        );
    }

    #[test]
    fn fields_for_payload_variant_returns_its_fields() {
        let schema = action_schema();
        let f = fields_for(&schema, "x:Action", Some("ClassifySpam")).unwrap();
        assert!(f.properties.contains_key("message"));
        assert_eq!(f.properties.len(), 1);
    }

    #[test]
    fn fields_for_no_variant_merges_all_variants() {
        let schema = action_schema();
        let f = fields_for(&schema, "x:Action", None).unwrap();
        assert!(f.properties.contains_key("message"));
    }

    #[test]
    fn fields_for_unknown_variant_errors() {
        let schema = action_schema();
        let err = fields_for(&schema, "x:Action", Some("DoesNotExist")).unwrap_err();
        assert!(format!("{err}").contains("DoesNotExist"));
    }
}
