/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::context::Context;
use crate::app::error::{CliError, CliResult};
use crate::cli::DeleteArgs;
use crate::jmap::Jmap;
use crate::jmap::errors::SetError;
use crate::render::Ansi;
use crate::render::set_error;
use crate::schema::ObjectType;
use crate::schema::resolve;
use crate::util::input;
use serde_json::{Value, json};
use std::io::Write;

pub fn run(ctx: &Context, args: &DeleteArgs) -> CliResult<()> {
    resolve::forbid_slash_form(&args.object)?;
    let canonical = resolve::require_object(&ctx.schema, &args.object)?;

    if matches!(
        ctx.schema.objects.get(canonical),
        Some(ObjectType::Singleton { .. })
    ) {
        return Err(CliError::msg(format!(
            "{} is a singleton and cannot be deleted",
            resolve::display_name(canonical)
        )));
    }

    let ids = resolve_ids(args)?;
    if ids.is_empty() {
        return Err(CliError::msg("no ids provided"));
    }

    let jmap = Jmap::new(&ctx.client, &ctx.session.api_path);
    let method = format!("{canonical}/set");
    let batch_size = ctx.session.max_objects_in_set.max(1);
    let ansi = Ansi::new(ctx.config.color);

    let mut deleted_count = 0usize;
    let mut failed_count = 0usize;
    let mut any_failure = false;
    let mut stdout = std::io::stdout().lock();

    for chunk in ids.chunks(batch_size) {
        let result = jmap.call(
            &method,
            json!({ "destroy": chunk.iter().map(|s| Value::String(s.clone())).collect::<Vec<_>>() }),
        )?;
        let destroyed: Vec<String> = result
            .get("destroyed")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let not_destroyed = result.get("notDestroyed").and_then(Value::as_object);

        for id in chunk {
            if destroyed.iter().any(|d| d == id) {
                writeln!(stdout, "{}{}{} deleted", ansi.green(), id, ansi.reset())?;
                deleted_count += 1;
            } else if let Some(err_val) = not_destroyed.and_then(|m| m.get(id)) {
                let set_err: SetError = serde_json::from_value(err_val.clone()).unwrap_or_default();
                writeln!(
                    stdout,
                    "{}{}{} failed: {}",
                    ansi.red(),
                    id,
                    ansi.reset(),
                    set_error::render(&set_err, Ansi::new(false)).replace('\n', "\n  ")
                )?;
                failed_count += 1;
                any_failure = true;
            } else {
                writeln!(
                    stdout,
                    "{}{}{} failed: no response from server",
                    ansi.red(),
                    id,
                    ansi.reset()
                )?;
                failed_count += 1;
                any_failure = true;
            }
        }
    }

    writeln!(stdout, "{} deleted, {} failed", deleted_count, failed_count)?;
    if any_failure {
        return Err(CliError::msg("one or more deletions failed"));
    }
    Ok(())
}

fn resolve_ids(args: &DeleteArgs) -> CliResult<Vec<String>> {
    match (&args.ids, args.stdin) {
        (Some(_), true) => Err(CliError::msg("--ids and --stdin are mutually exclusive")),
        (Some(s), false) => Ok(s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()),
        (None, true) => input::parse_ids_from_stdin(),
        (None, false) => Err(CliError::msg("either --ids or --stdin is required")),
    }
}
