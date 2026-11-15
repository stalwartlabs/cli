/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use cli::args::{Cli, Command};
use cli::commands;
use cli::config::{ColorRequest, Config};
use cli::context::{Context, report_store_warnings};
use cli::message::report_error;
use error::{Error, Result};
use jmap::http::HTTP_TRACE;
use log::{LevelFilter, Log, Metadata, Record};
use std::fmt::Arguments;
use std::io::{self, Write};
use tui::Launch;

mod auth;
mod cli;
mod error;
mod input;
mod jmap;
mod plan;
mod schema;
#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../cli-tests/unit/src/test_dir.rs"]
mod test_dir;
mod tui;
mod view;

fn main() {
    let parsed = Cli::parse_with_banner();
    let err_color = ColorRequest::of(&parsed.global).mode_for(&io::stderr());
    let code = match run(parsed) {
        Ok(()) => 0,
        Err(e) => {
            if is_broken_pipe(&e) {
                return;
            }
            report_error(&e, err_color);
            1
        }
    };
    std::process::exit(code);
}

fn is_broken_pipe(e: &Error) -> bool {
    matches!(e, Error::Io(io) if io.kind() == io::ErrorKind::BrokenPipe)
}

fn run(parsed: Cli) -> Result<()> {
    let command = match parsed.command {
        Some(Command::Cli(command)) => command,
        Some(Command::Logout) => return commands::logout::run(&parsed.global),
        Some(Command::Tui(args)) => {
            return tui::run(Launch::from_args(&parsed.global, Some(&args))?);
        }
        None if tui::is_interactive() => return tui::run(Launch::from_args(&parsed.global, None)?),
        None => return Ok(Cli::print_help()?),
    };
    let config = Config::resolve(&parsed.global)?;
    DebugLogger::install(config.log_level);
    let ctx = Context::init(&config)?;
    let result = commands::dispatch(ctx, command);
    report_store_warnings(&config.connect.credential, config.err_color);
    result
}

struct DebugLogger;

impl DebugLogger {
    fn install(level: LevelFilter) {
        if level != LevelFilter::Off && log::set_logger(&DebugLogger).is_ok() {
            log::set_max_level(level);
        }
    }

    fn write_line(out: &mut impl Write, args: &Arguments<'_>) -> io::Result<()> {
        writeln!(out, "[debug] {args}")
    }
}

impl Log for DebugLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target() == HTTP_TRACE
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let _ = Self::write_line(&mut io::stderr().lock(), record.args());
        }
    }

    fn flush(&self) {}
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../cli-tests/unit/src/tests.rs"]
mod tests;
