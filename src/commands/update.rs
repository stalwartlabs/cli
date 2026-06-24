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

    let outcome = interpret_update_response(&result, &id, canonical)?;
    let updated = match outcome {
        UpdateOutcome::Failed(err_val) => {
            let set_err: SetError = serde_json::from_value(err_val)?;
            let ansi = Ansi::new(ctx.config.color);
            eprintln!("{}", set_error::render(&set_err, ansi));
            return Err(CliError::msg("update failed"));
        }
        UpdateOutcome::Ok(v) => v,
    };

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

#[derive(Debug)]
enum UpdateOutcome {
    Ok(Option<Value>),
    Failed(Value),
}

fn interpret_update_response(
    result: &Value,
    id: &str,
    canonical: &str,
) -> CliResult<UpdateOutcome> {
    if let Some(not_updated) = result.get("notUpdated").and_then(Value::as_object)
        && let Some(err_val) = not_updated.get(id)
    {
        return Ok(UpdateOutcome::Failed(err_val.clone()));
    }
    let updated_map = result.get("updated").and_then(Value::as_object);
    match updated_map.and_then(|m| m.get(id)) {
        Some(v) => Ok(UpdateOutcome::Ok(Some(v.clone()))),
        None => Err(CliError::msg(format!(
            "{}: server did not acknowledge update for id `{id}` (the id may not exist)",
            resolve::display_name(canonical),
        ))),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_with_object_extras() -> CliResult<()> {
        let resp = json!({ "updated": { "abc": { "secretKey": "k" } } });
        let outcome = interpret_update_response(&resp, "abc", "x:Domain")?;
        match outcome {
            UpdateOutcome::Ok(Some(Value::Object(m))) => {
                assert_eq!(m.get("secretKey").and_then(Value::as_str), Some("k"));
            }
            _ => return Err(CliError::msg("expected Ok with object")),
        }
        Ok(())
    }

    #[test]
    fn ok_with_null_extras() -> CliResult<()> {
        let resp = json!({ "updated": { "abc": null } });
        let outcome = interpret_update_response(&resp, "abc", "x:Domain")?;
        assert!(matches!(outcome, UpdateOutcome::Ok(Some(Value::Null))));
        Ok(())
    }

    #[test]
    fn failed_returns_set_error_value() -> CliResult<()> {
        let resp = json!({
            "notUpdated": { "abc": { "type": "invalidProperties" } }
        });
        let outcome = interpret_update_response(&resp, "abc", "x:Domain")?;
        match outcome {
            UpdateOutcome::Failed(v) => {
                assert_eq!(
                    v.get("type").and_then(Value::as_str),
                    Some("invalidProperties")
                );
            }
            _ => return Err(CliError::msg("expected Failed")),
        }
        Ok(())
    }

    #[test]
    fn missing_id_in_both_maps_errors() {
        let resp = json!({ "updated": { "other": null }, "notUpdated": {} });
        let result = interpret_update_response(&resp, "abc", "x:Domain");
        let err = result.expect_err("expected Err");
        let msg = format!("{err}");
        assert!(msg.contains("did not acknowledge"));
        assert!(msg.contains("abc"));
    }

    #[test]
    fn empty_response_errors() {
        let resp = json!({});
        let err = interpret_update_response(&resp, "abc", "x:Domain");
        assert!(err.is_err());
    }
}
