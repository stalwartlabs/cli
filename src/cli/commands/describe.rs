/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::args::DescribeArgs;
use crate::cli::commands::CommandError;
use crate::cli::context::Context;
use crate::cli::describe::{Describer, unknown_describe_target};
use crate::cli::term::Ansi;
use crate::error::Result;
use std::io::{self, Write};

pub fn run(ctx: &Context, args: &DescribeArgs) -> Result<()> {
    let describer = Describer::new(&ctx.schema, Ansi::new(ctx.color));
    let out = match &args.name {
        None => describer.list_all(),
        Some(name) => match ctx.schema.object_for_describe(name)? {
            Some(def) => describer.object(def),
            None => match ctx.schema.enum_ci(name) {
                Some((enum_name, variants)) => describer.enumeration(enum_name, variants),
                None => {
                    io::stderr().write_all(unknown_describe_target(name).as_bytes())?;
                    return Err(CommandError::UnknownDescribeTarget(name.clone()).into());
                }
            },
        },
    };
    io::stdout().write_all(out.as_bytes())?;
    Ok(())
}
