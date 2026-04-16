/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::config::Config;
use crate::app::error::CliResult;
use crate::jmap::cache::SchemaCache;
use crate::jmap::http::{HttpClient, fetch_schema};
use crate::jmap::session::Session;
use crate::schema::Schema;

pub struct Context {
    pub config: Config,
    pub client: HttpClient,
    pub schema: Schema,
    pub session: Session,
}

impl Context {
    pub fn init(config: Config) -> CliResult<Self> {
        let client = HttpClient::new(&config)?;
        let cache = SchemaCache::new(&config.url)?;
        let schema = fetch_schema(&client, &cache, config.color)?;
        let session = Session::fetch(&client)?;
        Ok(Context {
            config,
            client,
            schema,
            session,
        })
    }
}
