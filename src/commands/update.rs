/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::context::Context;
use crate::app::error::{CliError, CliResult};
use crate::cli::UpdateArgs;
use crate::jmap::Jmap;
use crate::jmap::errors::SetError;
use crate::render::Ansi;
use crate::render::set_error;
use crate::schema::resolve;
use crate::schema::{Fields, ObjectSchema, ObjectType};
use crate::util::input::{self, Mode, Sources};
use serde_json::{Value, json};
use std::io::Write;

pub fn run(ctx: &Context, args: &UpdateArgs) -> CliResult<()> {
    resolve::forbid_slash_form(&args.object)?;
    let canonical = resolve::require_object(&ctx.schema, &args.object)?;

    let is_singleton = matches!(
        ctx.schema.objects.get(canonical),
        Some(ObjectType::Singleton { .. })
    );
    let id = if is_singleton {
        match args.id.as_deref() {
            None | Some("singleton") => "singleton".to_string(),
            Some(_) => return Err(CliError::BadSingletonId),
        }
    } else {
        match args.id.as_deref() {
            Some(v) if !v.is_empty() => v.to_string(),
            _ => {
                return Err(CliError::IdRequired(
                    resolve::display_name(canonical).to_string(),
                ));
            }
        }
    };

    let fields = union_or_single_fields(&ctx.schema, canonical);

    let sources = Sources {
        fields: &args.field,
        json: args.json.as_deref(),
        file: args.file.as_deref(),
        stdin: args.stdin,
    };
    let patch = input::build_input(&sources, &fields, Mode::Update)?;

    if patch.is_empty() {
        return Err(CliError::msg("no fields to update"));
    }

    let jmap = Jmap::new(&ctx.client, &ctx.session.api_path);
    let method = format!("{canonical}/set");
    let result = jmap.call(
        &method,
        json!({
            "update": { &id: Value::Object(patch) },
        }),
    )?;

    if let Some(not_updated) = result.get("notUpdated").and_then(Value::as_object)
        && let Some(err_val) = not_updated.get(&id)
    {
        let set_err: SetError = serde_json::from_value(err_val.clone())?;
        let ansi = Ansi::new(ctx.config.color);
        eprintln!("{}", set_error::render(&set_err, ansi));
        return Err(CliError::msg("update failed"));
    }

    let updated = result
        .get("updated")
        .and_then(Value::as_object)
        .and_then(|m| m.get(&id));

    let ansi = Ansi::new(ctx.config.color);
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{}Updated{} {} {}{}{}",
        ansi.green(),
        ansi.reset(),
        resolve::display_name(canonical),
        ansi.cyan(),
        id,
        ansi.reset(),
    )?;

    if let Some(Value::Object(extras)) = updated
        && !extras.is_empty()
    {
        writeln!(stdout)?;

        for (k, v) in extras {
            writeln!(stdout, "  {k}: {}", v)?;
        }
    }
    Ok(())
}

fn union_or_single_fields(schema: &crate::schema::Schema, canonical: &str) -> Fields {
    let Some(obj_schema) = schema.schemas.get(canonical) else {
        return Fields::default();
    };
    match obj_schema {
        ObjectSchema::Single { schema_name } => {
            schema.fields.get(schema_name).cloned().unwrap_or_default()
        }
        ObjectSchema::Multiple { variants } => {
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
            out
        }
    }
}
