/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::error::{CliError, CliResult};
use crate::cli::GlobalArgs;
use is_terminal::IsTerminal;

pub struct Config {
    pub url: String,
    pub auth: AuthMode,
    pub insecure: bool,
    pub color: bool,
}

pub enum AuthMode {
    Basic { user: String, password: String },
    Bearer { token: String },
}

impl Config {
    pub fn resolve(args: &GlobalArgs) -> CliResult<Self> {
        let url = args
            .url
            .clone()
            .ok_or(CliError::MissingUrl)?
            .trim_end_matches('/')
            .to_string();

        let auth = resolve_auth(args)?;

        let color = !args.no_color
            && std::env::var_os("NO_COLOR").is_none()
            && std::io::stdout().is_terminal();

        Ok(Config {
            url,
            auth,
            insecure: args.insecure,
            color,
        })
    }
}

fn resolve_auth(args: &GlobalArgs) -> CliResult<AuthMode> {
    if args.api_key.is_some() && args.user.is_some() {
        return Err(CliError::ConflictingCredentials);
    }
    if let Some(token) = &args.api_key {
        return Ok(AuthMode::Bearer {
            token: token.clone(),
        });
    }
    if let Some(user) = &args.user {
        let password = match &args.password {
            Some(p) => p.clone(),
            None => {
                if std::io::stdin().is_terminal() {
                    rpassword::prompt_password("Password: ")?
                } else {
                    return Err(CliError::NoPasswordNoTty);
                }
            }
        };
        return Ok(AuthMode::Basic {
            user: user.clone(),
            password,
        });
    }
    Err(CliError::MissingCredentials)
}
