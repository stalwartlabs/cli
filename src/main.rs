/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use clap::Parser;
use std::io::Write;

mod app;
mod cli;
mod commands;
mod jmap;
mod render;
mod schema;
mod util;

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(e) => {
            if is_broken_pipe(&e) {
                return;
            }
            report_error(&e);
            1
        }
    };
    std::process::exit(code);
}

fn is_broken_pipe(e: &app::CliError) -> bool {
    matches!(e, app::CliError::Io(io) if io.kind() == std::io::ErrorKind::BrokenPipe)
}

fn run() -> app::CliResult<()> {
    let parsed = cli::Cli::parse();
    let config = app::Config::resolve(&parsed.global)?;
    let ctx = app::Context::init(config)?;
    commands::dispatch(ctx, parsed.command)
}

fn report_error(e: &app::CliError) {
    let stderr = std::io::stderr();
    let mut w = stderr.lock();
    let is_tty = is_terminal::IsTerminal::is_terminal(&std::io::stderr());
    let colored = is_tty && std::env::var_os("NO_COLOR").is_none();
    let (red, reset) = if colored {
        ("\x1b[31m", "\x1b[0m")
    } else {
        ("", "")
    };
    let _ = writeln!(w, "{red}error:{reset} {e}");
}
