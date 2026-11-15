/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::args::CreateArgs;
use crate::cli::commands::{
    CommandError, Outcome, Source, TextInput, paint_object, print_outcome, report_set_error,
};
use crate::cli::context::Context;
use crate::cli::term::{Ansi, Heading};
use crate::error::{Error, Result};
use crate::input::Mode;
use serde_json::{Value, json};
use std::io::{self, Write};

const CREATE_ID: &str = "n0";
const TYPE_KEY: &str = "@type";

pub fn run(ctx: &Context, args: &CreateArgs) -> Result<()> {
    let (def, variant) = ctx.schema.create_target(&args.object)?;
    let fields = variant.map_or_else(|| def.all_fields(), |v| def.variant_fields(v));
    let source = Source::select([
        Source::fields(&args.field),
        args.json.as_deref().map(Source::Json),
        Source::file(args.file.as_deref()),
        args.stdin.then_some(Source::Text(TextInput::Stdin)),
    ])?;
    let mut input = source.payload(fields, Mode::Create)?;

    if let Some(variant) = variant {
        match input.get(TYPE_KEY) {
            Some(existing) if existing.as_str() != Some(variant.name.as_str()) => {
                eprintln!(
                    "{} --field `@type` overrides the Object/Variant shorthand",
                    Ansi::new(ctx.err_color).warning_tag()
                );
            }
            Some(_) => {}
            None => {
                input.insert(TYPE_KEY.to_owned(), Value::String(variant.name.clone()));
            }
        }
    }

    let mut response = ctx
        .client
        .set(def.name(), &json!({ "create": { CREATE_ID: input } }))?;

    if let Some((_, error)) = response.not_created.iter().find(|(id, _)| *id == CREATE_ID) {
        report_set_error(Ansi::new(ctx.err_color), error);
        return Err(CommandError::CreateFailed.into());
    }

    let Some(Value::Object(mut created)) = response.created.remove(CREATE_ID) else {
        return Err(Error::UnexpectedResponse(
            "set response missing created[n0]".into(),
        ));
    };
    let Some(Value::String(id)) = created.remove("id") else {
        return Err(Error::UnexpectedResponse(
            "created object missing id".into(),
        ));
    };

    let ansi = Ansi::new(ctx.color);
    let mut stdout = io::stdout().lock();
    print_outcome(&mut stdout, ansi, Outcome::Created, def, &id)?;
    if !created.is_empty() {
        writeln!(stdout)?;
        let mut out = String::new();
        paint_object(ctx, def, created, Heading::Untitled, &mut out);
        stdout.write_all(out.as_bytes())?;
    }
    Ok(())
}
