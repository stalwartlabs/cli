/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::login::BasicReason;
use crate::cli::term::{Ansi, ColorMode};
use crate::error::{CommandError, Error};
use crate::input::InputError;
use std::fmt::{self, Display, Formatter};
use std::io::{self, Write};

pub struct CliMessage<'a>(pub &'a Error);

pub fn report_error(e: &Error, color: ColorMode) {
    let _ = write_error(&mut io::stderr().lock(), e, color);
}

fn write_error(out: &mut impl Write, e: &Error, color: ColorMode) -> io::Result<()> {
    writeln!(out, "{} {}", Ansi::new(color).error_tag(), CliMessage(e))
}

impl Display for CliMessage<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            Error::MissingUrl => f.write_str("no URL provided (use --url or STALWART_URL)"),
            Error::MissingCredentials => f.write_str(
                "no credentials provided (use --user / --password, --api-key or --auth device)",
            ),
            Error::DeviceUnavailable(reason) => {
                f.write_str("no credentials provided and device sign-in is unavailable: ")?;
                basic_reason(reason, f)?;
                f.write_str(" (use --user / --password or --api-key)")
            }
            Error::ConflictingCredentials => {
                f.write_str("--user and --api-key are mutually exclusive")
            }
            Error::Command(e) => command(e, f),
            Error::Input(e) => input(e, f),
            other => Display::fmt(other, f),
        }
    }
}

fn basic_reason(reason: &BasicReason, f: &mut Formatter<'_>) -> fmt::Result {
    match reason {
        BasicReason::Preferred => f.write_str("basic authentication was requested"),
        BasicReason::Recovery => f.write_str("the server is in recovery mode"),
        BasicReason::NoDeviceSupport => f.write_str("the server does not support the device flow"),
        BasicReason::NoClientId => f.write_str("no OAuth client is available, pass --client-id"),
        BasicReason::DcrRefused(error) => write!(f, "client registration refused: {error}"),
        BasicReason::Discovery(error) => write!(f, "sign-in discovery failed: {error}"),
        BasicReason::External { issuer, reason } => {
            write!(f, "identity provider {issuer}: ")?;
            basic_reason(reason, f)
        }
    }
}

fn command(e: &CommandError, f: &mut Formatter<'_>) -> fmt::Result {
    f.write_str(match e {
        CommandError::ConflictingIds => "--ids and --stdin are mutually exclusive",
        CommandError::MissingIds => "either --ids or --stdin is required",
        CommandError::NoInput => "no input provided; use --field, --json, --file, or --stdin",
        CommandError::ConflictingInput => {
            "input sources are mutually exclusive: pick one of --field, --json, --file, --stdin"
        }
        CommandError::ConflictingPlanInput => "--file and --stdin are mutually exclusive",
        CommandError::NoPlan => "no plan provided; pass --file <path> or --stdin",
        other => return Display::fmt(other, f),
    })
}

fn input(e: &InputError, f: &mut Formatter<'_>) -> fmt::Result {
    match e {
        InputError::NestedCreateKey(key) => write!(
            f,
            "`--field` keys for `create` must be top-level properties (got `{key}`)"
        ),
        InputError::DuplicateField(key) => write!(f, "duplicate --field key `{key}`"),
        InputError::UnterminatedQuote(raw) => {
            write!(f, "invalid --field `{raw}`: unterminated quoted key")
        }
        InputError::EmptyQuotedKey(raw) => write!(f, "invalid --field `{raw}`: empty quoted key"),
        InputError::MissingEqualsAfterQuote(raw) => {
            write!(f, "invalid --field `{raw}`: expected `=` after quoted key")
        }
        InputError::MissingEquals(raw) => write!(f, "invalid --field `{raw}`: expected KEY=VALUE"),
        InputError::InvalidFieldJson(err) => write!(f, "invalid JSON in --field value: {err}"),
        InputError::InvalidWhere(raw) => write!(
            f,
            "invalid --where `{raw}`: expected field=value (or field>=value, <=, >, <)"
        ),
        other => Display::fmt(other, f),
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/cli/message/tests.rs"]
mod tests;
