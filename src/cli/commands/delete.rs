/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::args::DeleteArgs;
use crate::cli::commands::{CommandError, TextInput};
use crate::cli::context::Context;
use crate::cli::term::Ansi;
use crate::error::Result;
use crate::input::parse_ids;
use crate::jmap::set_error::SetError;
use crate::view::{RefLabels, Styled, set_error_body};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};

#[derive(Serialize)]
struct Destroy<'a> {
    destroy: &'a [String],
}

pub fn run(ctx: &Context, args: &DeleteArgs) -> Result<()> {
    let def = ctx.schema.object_no_variant(&args.object)?;
    if def.is_singleton() {
        return Err(CommandError::SingletonDelete(def.display_name().to_owned()).into());
    }

    let ids = ids(args)?;
    if ids.is_empty() {
        return Err(CommandError::NoIds.into());
    }

    let ansi = Ansi::new(ctx.color);
    let mut deleted = 0usize;
    let mut failed = 0usize;
    let mut stdout = io::stdout().lock();
    let object = def.display_name();

    for chunk in ids.chunks(ctx.client.max_set()) {
        let response = ctx.client.set(def.name(), &Destroy { destroy: chunk })?;
        let destroyed: HashSet<&str> = response.destroyed.iter().map(String::as_str).collect();
        let failures: HashMap<&str, &SetError> = response.not_destroyed.iter().collect();

        for id in chunk {
            if destroyed.contains(id.as_str()) {
                writeln!(
                    stdout,
                    "{}✓{} Deleted {object} {}{id}{}",
                    ansi.green(),
                    ansi.reset(),
                    ansi.cyan(),
                    ansi.reset()
                )?;
                deleted += 1;
                continue;
            }
            failed += 1;
            write!(
                stdout,
                "{}✗{} {object} {}{id}{}: ",
                ansi.red(),
                ansi.reset(),
                ansi.cyan(),
                ansi.reset()
            )?;
            match failures.get(id.as_str()) {
                Some(error) => {
                    let mut body = Styled::default();
                    set_error_body(error, &RefLabels::default(), &mut body);
                    let mut line = String::new();
                    ansi.paint(&body, &mut line);
                    writeln!(stdout, "{}", line.replace('\n', "\n  "))?;
                }
                None => writeln!(stdout, "no response from server")?,
            }
        }
    }

    writeln!(stdout, "{deleted} deleted, {failed} failed")?;
    if failed > 0 {
        return Err(CommandError::DeleteFailed.into());
    }
    Ok(())
}

fn ids(args: &DeleteArgs) -> Result<Vec<String>> {
    match (&args.ids, args.stdin) {
        (Some(_), true) => Err(CommandError::ConflictingIds.into()),
        (Some(raw), false) => Ok(raw
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect()),
        (None, true) => parse_ids(&TextInput::Stdin.read()?),
        (None, false) => Err(CommandError::MissingIds.into()),
    }
}
