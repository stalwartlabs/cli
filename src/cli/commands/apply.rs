/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::args::ApplyArgs;
use crate::cli::commands::{CommandError, TextInput};
use crate::cli::context::Context;
use crate::cli::report::{ApplyReporter, OutputFormat, Progress, Verbosity};
use crate::cli::term::Ansi;
use crate::error::Result;
use crate::plan::{ApplyError, ApplyOptions, OnError, Plan};
use std::io;

pub fn run(ctx: &Context, args: &ApplyArgs) -> Result<()> {
    let input = read_input(args)?;
    let plan = Plan::parse(&ctx.schema, &input)?;

    let mut out = io::stdout();
    let mut err = io::stderr();
    let mut reporter = ApplyReporter::new(
        Ansi::new(ctx.err_color),
        if args.quiet {
            Verbosity::Quiet
        } else {
            Verbosity::Normal
        },
        if args.json {
            OutputFormat::Json
        } else {
            OutputFormat::Human
        },
        if args.progress {
            Progress::Shown
        } else {
            Progress::Hidden
        },
        &mut out,
        &mut err,
    );
    reporter.plan_header(plan.stats());
    if args.dry_run {
        reporter.dry_run_note();
        return Ok(());
    }

    let opts = ApplyOptions {
        on_error: if args.continue_on_error {
            OnError::Continue
        } else {
            OnError::Stop
        },
    };
    let summary = plan.apply(&ctx.client, opts, &mut |event| reporter.event(event))?;
    if summary.failed > 0 {
        return Err(ApplyError::Incomplete {
            failed: summary.failed,
        }
        .into());
    }
    Ok(())
}

fn read_input(args: &ApplyArgs) -> Result<String> {
    match (&args.file, args.stdin) {
        (Some(path), false) => TextInput::File(path).read(),
        (None, true) => TextInput::Stdin.read(),
        (Some(_), true) => Err(CommandError::ConflictingPlanInput.into()),
        (None, false) => Err(CommandError::NoPlan.into()),
    }
}
