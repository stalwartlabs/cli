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
pub mod query;
pub mod update;

use crate::app::context::Context;
use crate::app::error::CliResult;
use crate::cli::Command;

pub fn dispatch(ctx: Context, cmd: Command) -> CliResult<()> {
    match cmd {
        Command::Describe(args) => describe::run(&ctx, &args),
        Command::Get(args) => get::run(&ctx, &args),
        Command::Query(args) => query::run(&ctx, &args),
        Command::Create(args) => create::run(&ctx, &args),
        Command::Update(args) => update::run(&ctx, &args),
        Command::Delete(args) => delete::run(&ctx, &args),
        Command::Apply(args) => apply::run(&ctx, &args),
    }
}
