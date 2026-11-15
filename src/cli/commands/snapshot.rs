/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::args::SnapshotArgs;
use crate::cli::context::Context;
use crate::cli::report::{SnapshotReporter, Verbosity};
use crate::cli::term::Ansi;
use crate::error::Result;
use crate::plan::{Secrets, Snapshot};
use std::fs::File;
use std::io::{self, BufWriter, StdoutLock, Write};

enum Output {
    File(BufWriter<File>),
    Stdout(StdoutLock<'static>),
}

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Output::File(f) => f.write(buf),
            Output::Stdout(s) => s.write(buf),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Output::File(f) => f.write_all(buf),
            Output::Stdout(s) => s.write_all(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Output::File(f) => f.flush(),
            Output::Stdout(s) => s.flush(),
        }
    }
}

pub fn run(ctx: &Context, args: &SnapshotArgs) -> Result<()> {
    let snapshot = Snapshot::build(&ctx.schema, &args.objects, &args.allow_unresolved)?;

    let mut err = io::stderr();
    let mut reporter = SnapshotReporter::new(
        Ansi::new(ctx.err_color),
        if args.quiet {
            Verbosity::Quiet
        } else {
            Verbosity::Normal
        },
        &mut err,
    );
    reporter.plan_header(snapshot.stats());

    let mut output = match &args.output {
        Some(path) => Output::File(BufWriter::new(File::create(path)?)),
        None => Output::Stdout(io::stdout().lock()),
    };
    let secrets = if args.include_secrets {
        Secrets::Include
    } else {
        Secrets::Strip
    };
    snapshot.write(&ctx.client, secrets, &mut output, &mut |event| {
        reporter.event(event)
    })?;
    reporter.done();
    Ok(())
}
