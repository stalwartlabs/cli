/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::context::Context;
use crate::app::error::CliResult;
use crate::cli::DescribeArgs;
use crate::render::Ansi;
use crate::render::describe as render_describe;
use crate::schema::resolve;
use std::io::Write;

pub fn run(ctx: &Context, args: &DescribeArgs) -> CliResult<()> {
    let ansi = Ansi::new(ctx.config.color);
    let out = match &args.name {
        None => render_describe::list_all(&ctx.schema, ansi),
        Some(name) => {
            if let Some(canonical) = resolve::resolve_object(&ctx.schema, name, true)? {
                render_describe::describe_object(&ctx.schema, canonical, ansi)
            } else if let Some(enum_name) = resolve::resolve_enum(&ctx.schema, name) {
                render_describe::describe_enum(&ctx.schema, enum_name, ansi)
            } else {
                let msg = render_describe::unknown_describe_target(name);
                write_all(std::io::stderr(), msg.as_bytes())?;
                return Err(crate::app::error::CliError::msg(format!(
                    "no object or enum named `{name}`"
                )));
            }
        }
    };
    write_all(std::io::stdout(), out.as_bytes())?;
    Ok(())
}

fn write_all<W: Write>(mut w: W, bytes: &[u8]) -> std::io::Result<()> {
    w.write_all(bytes)?;
    Ok(())
}
