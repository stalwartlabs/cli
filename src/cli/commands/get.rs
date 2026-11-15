/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::args::GetArgs;
use crate::cli::commands::{CommandError, object_id, paint_object, property};
use crate::cli::context::Context;
use crate::cli::term::{Heading, write_json_line};
use crate::error::Result;
use crate::jmap::response::GetResponse;
use std::io::{self, Write};

pub fn run(ctx: &Context, args: &GetArgs) -> Result<()> {
    let def = ctx.schema.object_no_variant(&args.object)?;
    let id = object_id(def, args.id.as_deref())?;
    let fields = def.all_fields();
    let properties = args
        .fields
        .as_deref()
        .map(|raws| {
            raws.iter()
                .map(|raw| property(fields, raw).map(|(name, _)| name))
                .collect::<Result<Vec<&str>>>()
        })
        .transpose()?;

    let GetResponse { list } = ctx
        .client
        .get(def.name(), &[id.as_str()], properties.as_deref())?;
    let Some(obj) = list.into_iter().next() else {
        return Err(CommandError::NotFound {
            object: def.display_name().to_owned(),
            id: id.as_str().to_owned(),
        }
        .into());
    };

    if args.json {
        return write_json_line(&mut io::stdout().lock(), &obj);
    }
    let mut out = String::new();
    let heading = Heading::Titled {
        object: def.display_name(),
        id: (!def.is_singleton()).then(|| id.as_str()),
    };
    paint_object(ctx, def, obj, heading, &mut out);
    io::stdout().write_all(out.as_bytes())?;
    Ok(())
}
