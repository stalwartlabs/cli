/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::config::Config;
use crate::cli::message::CliMessage;
use crate::cli::pager::Paging;
use crate::cli::term::{Ansi, ColorMode};
use crate::error::{Error, Result};
use crate::jmap::Client;
use crate::jmap::cache::SchemaCache;
use crate::jmap::credential::Credential;
use crate::jmap::http::HttpClient;
use crate::jmap::schema_fetch::{self, FetchedSchema};
use crate::schema::Schema;
use std::io::{self, Write};
use std::process;

pub struct Context {
    pub client: Client,
    pub schema: Schema,
    pub color: ColorMode,
    pub err_color: ColorMode,
    pub paging: Paging,
}

impl Context {
    pub fn init(config: &Config) -> Result<Self> {
        let http = HttpClient::new(&config.connect)?;
        let cache_base = dirs::cache_dir().ok_or(Error::NoCacheDir)?;
        let cache = SchemaCache::new(&cache_base, &config.connect.url, process::id())?;
        let fetched = schema_fetch::fetch(&http, &cache);
        if let Some(refresh) = stale_refresh_error(&fetched) {
            let _ = write_stale_warning(&mut io::stderr().lock(), config.err_color, refresh);
        }
        let schema = fetched?.schema;
        Ok(Context {
            client: Client::connect(http)?,
            schema,
            color: config.color,
            err_color: config.err_color,
            paging: config.paging,
        })
    }
}

fn stale_refresh_error(fetched: &Result<FetchedSchema>) -> Option<&Error> {
    match fetched {
        Ok(fetched) => fetched.stale.as_ref(),
        Err(e) => e.refresh_failure(),
    }
}

fn write_stale_warning(out: &mut impl Write, color: ColorMode, refresh: &Error) -> io::Result<()> {
    let ansi = Ansi::new(color);
    writeln!(
        out,
        "{} using the cached schema, refreshing it failed: {}",
        ansi.warning_tag(),
        CliMessage(refresh),
    )
}

pub fn report_store_warnings(credential: &Credential, color: ColorMode) {
    let ansi = Ansi::new(color);
    let mut out = io::stderr().lock();
    for warning in credential.take_store_warnings() {
        let _ = writeln!(out, "{} {}", ansi.warning_tag(), warning.message());
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/cli/context/tests.rs"]
mod tests;
