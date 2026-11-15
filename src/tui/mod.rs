/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod app;
pub mod chart;
pub mod exec;
pub mod form;
pub mod job;
pub mod keymap;
pub mod log;
pub mod msg;
pub mod prefs;
pub mod route;
pub mod runtime;
pub mod screens;
pub mod session;
pub mod term;
pub mod theme;
pub mod widgets;

use crate::cli::args::{AuthMethod, GlobalArgs, SidebarChoice, ThemeChoice, TuiArgs};
use crate::cli::term::ColorMode;
use crate::error::{Error, Result};
use crate::jmap::http::TlsVerify;
use app::App;
use is_terminal::IsTerminal;
use log::HttpLog;
use prefs::Prefs;
use std::env;
use std::io;
use std::path::PathBuf;
use std::time::Instant;
use term::TerminalGuard;
use theme::{Palette, PaletteRequest};
use widgets::sidebar::SidebarMode;

pub enum StaticAuth {
    None,
    ApiKey(String),
    User {
        user: String,
        password: Option<String>,
    },
}

pub struct Launch {
    pub url: Option<String>,
    pub auth: StaticAuth,
    pub pref: Option<AuthMethod>,
    pub client_id: Option<String>,
    pub tls: TlsVerify,
    pub palette: PaletteRequest,
    pub sidebar: Option<SidebarMode>,
    pub debug: bool,
    pub cache_dir: Option<PathBuf>,
    pub auth_dir: Option<PathBuf>,
    pub prefs_path: Option<PathBuf>,
    pub writer_id: u32,
}

impl Launch {
    pub fn from_args(global: &GlobalArgs, tui: Option<&TuiArgs>) -> Result<Self> {
        let auth = match (&global.api_key, &global.user) {
            (Some(_), Some(_)) => return Err(Error::ConflictingCredentials),
            (Some(key), None) => StaticAuth::ApiKey(key.clone()),
            (None, Some(user)) => StaticAuth::User {
                user: user.clone(),
                password: global.password.clone(),
            },
            (None, None) => StaticAuth::None,
        };
        let colourless = global.no_color || ColorMode::disabled_by_env();
        let palette = match tui.and_then(|t| t.theme) {
            _ if colourless => PaletteRequest::Forced(Palette::Mono),
            Some(choice) => PaletteRequest::Forced(choice.into()),
            None => PaletteRequest::Default(terminal_palette()),
        };
        let sidebar = tui.and_then(|t| t.sidebar).map(|choice| match choice {
            SidebarChoice::Always => SidebarMode::Always,
            SidebarChoice::Auto => SidebarMode::Auto,
        });
        let data = dirs::data_local_dir().map(|d| d.join("stalwart-cli"));
        Ok(Launch {
            url: global.url.clone(),
            auth,
            pref: global.auth,
            client_id: global.client_id.clone(),
            tls: if global.insecure {
                TlsVerify::Insecure
            } else {
                TlsVerify::Verify
            },
            palette,
            sidebar,
            debug: global.debug || env::var_os("STALWART_DEBUG").is_some_and(|v| !v.is_empty()),
            cache_dir: dirs::cache_dir(),
            auth_dir: data.as_ref().map(|d| d.join("auth")),
            prefs_path: dirs::config_dir().map(|d| d.join("stalwart-cli").join("tui.json")),
            writer_id: std::process::id(),
        })
    }
}

impl From<ThemeChoice> for Palette {
    fn from(choice: ThemeChoice) -> Self {
        match choice {
            ThemeChoice::Dark => Palette::Dark,
            ThemeChoice::Light => Palette::Light,
            ThemeChoice::Ansi16 => Palette::Ansi16,
            ThemeChoice::Mono => Palette::Mono,
        }
    }
}

fn terminal_palette() -> Palette {
    match env::var("COLORTERM") {
        Ok(value) if value == "truecolor" || value == "24bit" => Palette::Dark,
        _ => Palette::Ansi16,
    }
}

pub fn is_interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

pub fn run(launch: Launch) -> Result<()> {
    if !is_interactive() {
        return Err(Error::Io(io::Error::other(
            "the interactive console needs a terminal on stdin and stdout",
        )));
    }
    let prefs = launch
        .prefs_path
        .as_deref()
        .map(Prefs::load)
        .unwrap_or_default();
    let mut app = App::new(launch, prefs, Instant::now());
    if app.launch.debug {
        let log = HttpLog::default();
        if log.install() {
            app.log = Some(log);
        }
    }
    let guard = TerminalGuard::enter()?;
    runtime::run(app, guard)
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/tui/tests.rs"]
mod tests;
