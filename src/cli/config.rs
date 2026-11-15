/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::login::AuthPreference;
use crate::cli::args::{AuthMethod, GlobalArgs};
use crate::cli::login;
use crate::cli::pager::Paging;
use crate::cli::term::ColorMode;
use crate::error::{Error, Result};
use crate::jmap::auth::Auth;
use crate::jmap::credential::Credential;
use crate::jmap::http::{ConnectOptions, TlsVerify};
use clap::ColorChoice;
use is_terminal::IsTerminal;
use log::LevelFilter;
use std::env;
use std::ffi::OsStr;
use std::io;
use std::sync::Arc;

const NO_COLOR_FLAG: &str = "--no-color";

pub struct Config {
    pub connect: ConnectOptions,
    pub color: ColorMode,
    pub err_color: ColorMode,
    pub paging: Paging,
    pub log_level: LevelFilter,
}

impl Config {
    pub fn resolve(args: &GlobalArgs) -> Result<Self> {
        let url = resolve_url(args)?;
        let tls = tls_of(args);
        let credential = Arc::new(resolve_auth(args, &url, tls)?);
        let log_level =
            if args.debug || env::var_os("STALWART_DEBUG").is_some_and(|v| !v.is_empty()) {
                LevelFilter::Debug
            } else {
                LevelFilter::Off
            };

        let request = ColorRequest::of(args);
        Ok(Config {
            connect: ConnectOptions {
                url,
                credential,
                tls,
            },
            color: request.mode_for(&io::stdout()),
            err_color: request.mode_for(&io::stderr()),
            paging: Paging::detect(&io::stdout()),
            log_level,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorRequest {
    Auto,
    Never,
}

impl ColorRequest {
    pub fn of(args: &GlobalArgs) -> Self {
        if args.no_color {
            ColorRequest::Never
        } else {
            ColorRequest::Auto
        }
    }

    pub fn before_parse() -> Self {
        if env::args_os().any(|arg| arg == NO_COLOR_FLAG) {
            ColorRequest::Never
        } else {
            ColorRequest::Auto
        }
    }

    pub fn mode_for(self, stream: &impl IsTerminal) -> ColorMode {
        match self {
            ColorRequest::Auto => ColorMode::detect(stream),
            ColorRequest::Never => ColorMode::Disabled,
        }
    }
}

impl ColorMode {
    fn detect(stream: &impl IsTerminal) -> Self {
        Self::from_env(
            stream,
            env::var_os("NO_COLOR").as_deref(),
            env::var_os("TERM").as_deref(),
        )
    }

    pub fn disabled_by_env() -> bool {
        Self::disabled_by(
            env::var_os("NO_COLOR").as_deref(),
            env::var_os("TERM").as_deref(),
        )
    }

    fn disabled_by(no_color: Option<&OsStr>, term: Option<&OsStr>) -> bool {
        no_color.is_some_and(|value| !value.is_empty()) || term.is_some_and(|term| term == "dumb")
    }

    fn from_env(stream: &impl IsTerminal, no_color: Option<&OsStr>, term: Option<&OsStr>) -> Self {
        if stream.is_terminal() && !Self::disabled_by(no_color, term) {
            ColorMode::Enabled
        } else {
            ColorMode::Disabled
        }
    }

    pub fn clap_choice(self) -> ColorChoice {
        match self {
            ColorMode::Enabled => ColorChoice::Auto,
            ColorMode::Disabled => ColorChoice::Never,
        }
    }
}

pub fn resolve_url(args: &GlobalArgs) -> Result<String> {
    let url = args
        .url
        .as_ref()
        .ok_or(Error::MissingUrl)?
        .trim_end_matches('/')
        .to_string();
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(Error::MissingUrlScheme(url));
    }
    Ok(url)
}

pub fn tls_of(args: &GlobalArgs) -> TlsVerify {
    if args.insecure {
        TlsVerify::Insecure
    } else {
        TlsVerify::Verify
    }
}

fn resolve_auth(args: &GlobalArgs, url: &str, tls: TlsVerify) -> Result<Credential> {
    if args.api_key.is_some() && args.user.is_some() {
        return Err(Error::ConflictingCredentials);
    }
    if let Some(token) = &args.api_key {
        return Credential::from_auth(&Auth::Bearer {
            token: token.clone(),
        });
    }
    if let Some(user) = &args.user {
        let password = match &args.password {
            Some(p) => p.clone(),
            None if io::stdin().is_terminal() => rpassword::prompt_password("Password: ")?,
            None => return Err(Error::NoPasswordNoTty),
        };
        return Credential::from_auth(&Auth::Basic {
            user: user.clone(),
            password,
        });
    }
    let pref = match args.auth {
        Some(AuthMethod::Basic) => return Err(Error::MissingCredentials),
        Some(AuthMethod::Device) => AuthPreference::Device,
        None => AuthPreference::Auto,
    };
    if let Some(credential) = login::stored_credential(url, pref)? {
        return Ok(credential);
    }
    match device_attempt(pref, &io::stderr()) {
        Some(pref) => login::device_login(url, tls, args.client_id.as_deref(), pref),
        None => Err(Error::MissingCredentials),
    }
}

fn device_attempt(pref: AuthPreference, stderr: &impl IsTerminal) -> Option<AuthPreference> {
    match pref {
        AuthPreference::Device => Some(pref),
        AuthPreference::Auto if stderr.is_terminal() => Some(pref),
        AuthPreference::Auto | AuthPreference::Basic => None,
    }
}

impl Paging {
    pub fn detect(stream: &impl IsTerminal) -> Self {
        if stream.is_terminal() {
            Paging::Interactive
        } else {
            Paging::Stream
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/cli/config/tests.rs"]
mod tests;
