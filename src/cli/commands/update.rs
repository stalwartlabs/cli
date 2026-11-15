/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::args::UpdateArgs;
use crate::cli::commands::{
    CommandError, Outcome, Source, TextInput, object_id, print_outcome, report_set_error,
};
use crate::cli::context::Context;
use crate::cli::term::Ansi;
use crate::error::Result;
use crate::input::Mode;
use crate::jmap::response::SetResponse;
use crate::jmap::set_error::SetError;
use serde_json::{Value, json};
use std::io::{self, Write};

#[derive(Debug)]
enum UpdateOutcome<'r> {
    Updated(Option<Value>),
    Failed(&'r SetError),
}

pub fn run(ctx: &Context, args: &UpdateArgs) -> Result<()> {
    let def = ctx.schema.object_no_variant(&args.object)?;
    let id = object_id(def, args.id.as_deref())?.as_str();
    let source = Source::select([
        Source::fields(&args.field),
        args.json.as_deref().map(Source::Json),
        Source::file(args.file.as_deref()),
        args.stdin.then_some(Source::Text(TextInput::Stdin)),
    ])?;
    let patch = source.payload(def.all_fields(), Mode::Update)?;
    if patch.is_empty() {
        return Err(CommandError::EmptyUpdate.into());
    }

    let mut response = ctx
        .client
        .set(def.name(), &json!({ "update": { id: patch } }))?;

    let extras = match outcome(&mut response, def.display_name(), id)? {
        UpdateOutcome::Failed(error) => {
            report_set_error(Ansi::new(ctx.err_color), error);
            return Err(CommandError::UpdateFailed.into());
        }
        UpdateOutcome::Updated(extras) => extras,
    };

    let ansi = Ansi::new(ctx.color);
    let mut stdout = io::stdout().lock();
    print_outcome(&mut stdout, ansi, Outcome::Updated, def, id)?;
    if let Some(Value::Object(extras)) = extras
        && !extras.is_empty()
    {
        writeln!(stdout)?;
        for (key, value) in extras {
            writeln!(stdout, "  {key}: {value}")?;
        }
    }
    Ok(())
}

fn outcome<'r>(response: &'r mut SetResponse, object: &str, id: &str) -> Result<UpdateOutcome<'r>> {
    if let Some((_, error)) = response.not_updated.iter().find(|(key, _)| *key == id) {
        return Ok(UpdateOutcome::Failed(error));
    }
    match response.updated.remove(id) {
        Some(extras) => Ok(UpdateOutcome::Updated(extras)),
        None => Err(CommandError::UpdateNotAcknowledged {
            object: object.to_owned(),
            id: id.to_owned(),
        }
        .into()),
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/cli/commands/update/tests.rs"]
mod tests;
