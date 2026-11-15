/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod apply;
pub mod create;
pub mod delete;
pub mod describe;
pub mod get;
pub mod logout;
pub mod query;
pub mod snapshot;
pub mod update;

pub use crate::error::CommandError;

use crate::cli::args::CliCommand;
use crate::cli::context::Context;
use crate::cli::term::{Ansi, Heading};
use crate::error::{Error, Result};
use crate::input::{Mode, Payload};
use crate::jmap::set_error::SetError;
use crate::schema::object::{FieldSet, IdError, ObjectId};
use crate::schema::{Field, ObjectDef};
use crate::view::{LabelCache, ObjectDoc, RefLabels, Styled, set_error};
use serde_json::{Map, Value};
use std::io::{self, Read, Write};
use std::path::Path;
use std::slice;

#[derive(Clone, Copy)]
enum Source<'a> {
    None,
    Fields(&'a [String]),
    Json(&'a str),
    Text(TextInput<'a>),
}

#[derive(Clone, Copy)]
pub enum TextInput<'a> {
    File(&'a Path),
    Stdin,
}

#[derive(Clone, Copy)]
enum Outcome {
    Created,
    Updated,
}

pub fn dispatch(ctx: Context, cmd: CliCommand) -> Result<()> {
    match cmd {
        CliCommand::Describe(args) => describe::run(&ctx, &args),
        CliCommand::Get(args) => get::run(&ctx, &args),
        CliCommand::Query(args) => query::run(&ctx, &args),
        CliCommand::Create(args) => create::run(&ctx, &args),
        CliCommand::Update(args) => update::run(&ctx, &args),
        CliCommand::Delete(args) => delete::run(&ctx, &args),
        CliCommand::Apply(args) => apply::run(&ctx, &args),
        CliCommand::Snapshot(args) => snapshot::run(&ctx, &args),
    }
}

impl<'a> Source<'a> {
    fn fields(entries: &'a [String]) -> Option<Self> {
        (!entries.is_empty()).then_some(Source::Fields(entries))
    }

    fn file(path: Option<&'a Path>) -> Option<Self> {
        path.map(|path| Source::Text(TextInput::File(path)))
    }

    fn select(candidates: [Option<Self>; 4]) -> Result<Self> {
        let mut sources = candidates.into_iter().flatten();
        let first = sources.next().unwrap_or(Source::None);
        match sources.next() {
            Some(_) => Err(CommandError::ConflictingInput.into()),
            None => Ok(first),
        }
    }

    fn payload(self, fields: FieldSet<'_>, mode: Mode) -> Result<Map<String, Value>> {
        let text: String;
        let payload = match self {
            Source::None if mode == Mode::Create && fields.is_empty() => return Ok(Map::new()),
            Source::None => return Err(CommandError::NoInput.into()),
            Source::Fields(entries) => Payload::Fields(entries),
            Source::Json(json) => Payload::Json(json),
            Source::Text(input) => {
                text = input.read()?;
                Payload::Json(&text)
            }
        };
        payload.build(fields, mode)
    }
}

impl TextInput<'_> {
    pub fn read(self) -> Result<String> {
        match self {
            TextInput::File(path) => String::from_utf8(std::fs::read(path)?).map_err(|err| {
                CommandError::InvalidUtf8 {
                    path: path.to_owned(),
                    reason: err.utf8_error(),
                }
                .into()
            }),
            TextInput::Stdin => {
                let mut text = String::new();
                io::stdin().read_to_string(&mut text)?;
                Ok(text)
            }
        }
    }
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Created => "Created",
            Outcome::Updated => "Updated",
        }
    }
}

fn object_id<'a>(def: ObjectDef<'a>, raw: Option<&'a str>) -> Result<ObjectId<'a>> {
    def.object_id(raw).map_err(|err| match err {
        IdError::BadSingleton => Error::BadSingletonId,
        IdError::Missing => Error::IdRequired(def.display_name().to_owned()),
    })
}

fn property<'a>(fields: FieldSet<'a>, raw: &str) -> Result<(&'a str, Option<&'a Field>)> {
    let raw = raw.trim();
    let special = hashify::map_ignore_case!(
        raw.as_bytes(),
        &'static str,
        "id" => "id",
        "@type" => "@type",
    );
    if let Some(name) = special {
        return Ok((name, None));
    }
    fields
        .get_ci(raw)
        .map(|(name, field)| (name, Some(field)))
        .ok_or_else(|| Error::UnknownField(raw.to_owned()))
}

fn print_outcome(
    out: &mut dyn Write,
    ansi: Ansi,
    outcome: Outcome,
    def: ObjectDef<'_>,
    id: &str,
) -> io::Result<()> {
    writeln!(
        out,
        "{}{}{} {} {}{id}{}",
        ansi.green(),
        outcome.as_str(),
        ansi.reset(),
        def.display_name(),
        ansi.cyan(),
        ansi.reset(),
    )
}

fn report_set_error(ansi: Ansi, error: &SetError) {
    let mut styled = Styled::default();
    set_error(error, &RefLabels::default(), &mut styled);
    ansi.eprintln(&styled);
}

fn paint_object(
    ctx: &Context,
    def: ObjectDef<'_>,
    obj: Map<String, Value>,
    heading: Heading<'_>,
    out: &mut String,
) {
    let mut labels = LabelCache::default();
    let fields = def.fields_of(&obj).map_or(FieldSet::Empty, FieldSet::One);
    labels.populate(&ctx.client, &ctx.schema, fields, slice::from_ref(&obj));
    let value = Value::Object(obj);
    let doc = ObjectDoc::build(&ctx.schema, &labels, def, &value);
    Ansi::new(ctx.color).paint_doc(&doc, heading, out);
}
