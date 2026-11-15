/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::store::{StoredSession, TokenStore};
use crate::error::{Error, Result};
use crate::jmap::Client;
use crate::jmap::account::AccountInfo;
use crate::jmap::auth::Auth;
use crate::jmap::cache::SchemaCache;
use crate::jmap::credential::{Credential, TokenSession};
use crate::jmap::http::{ConnectOptions, HttpClient, TlsVerify};
use crate::jmap::schema_fetch;
use crate::schema::Schema;
use crate::tui::exec::JobCx;
use std::path::PathBuf;
use std::sync::Arc;

pub enum Login {
    Static(Auth),
    Session(Box<StoredSession>),
}

pub struct ConnectRequest {
    pub url: String,
    pub login: Login,
    pub auth_dir: Option<PathBuf>,
    pub tls: TlsVerify,
    pub cache_dir: Option<PathBuf>,
    pub writer_id: u32,
}

pub struct Link {
    pub url: String,
    pub client: Client,
    pub schema: Schema,
    pub account: AccountInfo,
    pub credential: Arc<Credential>,
    pub stale: Option<Error>,
}

pub enum SessionJob {
    Connect(Box<ConnectRequest>),
}

pub enum SessionOutput {
    Connected(Result<Box<Link>, Box<Error>>),
}

impl SessionOutput {
    pub fn error(&self) -> Option<&Error> {
        match self {
            SessionOutput::Connected(result) => result.as_ref().err().map(AsRef::as_ref),
        }
    }
}

impl SessionJob {
    pub fn run(self, _cx: &JobCx) -> SessionOutput {
        match self {
            SessionJob::Connect(request) => {
                SessionOutput::Connected(connect(*request).map(Box::new).map_err(Box::new))
            }
        }
    }
}

fn connect(request: ConnectRequest) -> Result<Link> {
    let ConnectRequest {
        url,
        login,
        auth_dir,
        tls,
        cache_dir,
        writer_id,
    } = request;
    let cache_dir = cache_dir.ok_or(Error::NoCacheDir)?;
    let cache = SchemaCache::new(&cache_dir, &url, writer_id)?;
    let credential = Arc::new(match login {
        Login::Static(auth) => Credential::from_auth(&auth)?,
        Login::Session(stored) => Credential::oauth(TokenSession {
            stored: *stored,
            store: auth_dir.and_then(|dir| TokenStore::new(&dir, &url, writer_id).ok()),
        })?,
    });
    let http = HttpClient::new(&ConnectOptions {
        url: url.clone(),
        credential: credential.clone(),
        tls,
    })?;
    let fetched = schema_fetch::fetch(&http, &cache)?;
    let account = AccountInfo::fetch(&http)?;
    Ok(Link {
        url,
        client: Client::connect(http)?,
        schema: fetched.schema,
        account,
        credential,
        stale: fetched.stale,
    })
}
