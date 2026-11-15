/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::args::Cli;
use crate::cli::config::ColorRequest;
use crate::cli::term::ColorMode;
use clap::{Command, CommandFactory, FromArgMatches};
use std::io;
use terminal_size::{Width, terminal_size_of};

const LARGE_LOGO_COLUMNS: u16 = 91;
const LARGE_ANS: &str = include_str!("logo/large.ans").trim_ascii_end();
const LARGE_TXT: &str = include_str!("logo/large.txt").trim_ascii_end();
const COMPACT_ANS: &str = include_str!("logo/compact.ans").trim_ascii_end();
const COMPACT_TXT: &str = include_str!("logo/compact.txt").trim_ascii_end();

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Logo {
    Large,
    Compact,
}

impl Logo {
    fn fitting(columns: Option<u16>) -> Self {
        match columns {
            Some(columns) if columns >= LARGE_LOGO_COLUMNS => Logo::Large,
            _ => Logo::Compact,
        }
    }

    const fn text(self, color: ColorMode) -> &'static str {
        match (self, color) {
            (Logo::Large, ColorMode::Enabled) => LARGE_ANS,
            (Logo::Large, ColorMode::Disabled) => LARGE_TXT,
            (Logo::Compact, ColorMode::Enabled) => COMPACT_ANS,
            (Logo::Compact, ColorMode::Disabled) => COMPACT_TXT,
        }
    }
}

impl Cli {
    pub fn parse_with_banner() -> Self {
        let matches = Cli::command_with_banner().get_matches();
        Cli::from_arg_matches(&matches).unwrap_or_else(|err| err.exit())
    }

    pub fn print_help() -> io::Result<()> {
        Cli::command_with_banner().print_help()
    }

    fn command_with_banner() -> Command {
        let color = ColorRequest::before_parse().mode_for(&io::stdout());
        let columns = terminal_size_of(io::stdout()).map(|(Width(columns), _)| columns);
        let logo = Logo::fitting(columns).text(color);
        Cli::command()
            .color(color.clap_choice())
            .before_help(logo)
            .before_long_help(logo)
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/cli/logo/tests.rs"]
mod tests;
