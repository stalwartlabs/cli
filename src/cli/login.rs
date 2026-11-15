/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::AuthError;
use crate::auth::device::Poll;
use crate::auth::discover::{Discovery, IdpMetadata};
use crate::auth::login::{self, AuthPreference, BasicReason, LoginStep};
use crate::auth::store::TokenStore;
use crate::error::{Error, Result};
use crate::jmap::credential::{Credential, TokenSession};
use crate::jmap::http::{HttpClient, TlsVerify};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::SystemTime;
use std::{process, thread};

const APP_DIR: &str = "stalwart-cli";
const AUTH_DIR: &str = "auth";

pub fn auth_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join(APP_DIR).join(AUTH_DIR))
}

pub fn token_store(url: &str) -> Option<TokenStore> {
    TokenStore::new(&auth_dir()?, url, process::id()).ok()
}

pub fn stored_credential(url: &str, pref: AuthPreference) -> Result<Option<Credential>> {
    let store = token_store(url);
    let Some(stored) = login::stored_session(store.as_ref(), None, pref, SystemTime::now()) else {
        return Ok(None);
    };
    Credential::oauth(TokenSession { stored, store }).map(Some)
}

pub fn device_login(
    url: &str,
    tls: TlsVerify,
    client_id: Option<&str>,
    pref: AuthPreference,
) -> Result<Credential> {
    let store = token_store(url);
    let http = HttpClient::anonymous(url, tls)?;
    let step = login::plan(
        url,
        Discovery::fetch(&http, None),
        pref,
        client_id,
        |endpoint| store.as_ref()?.cached_client(endpoint),
    )?;
    let mut plan = match step {
        LoginStep::Device(plan) => plan,
        LoginStep::External {
            issuer,
            client_id,
            scope,
            resource,
        } => {
            let idp = match IdpMetadata::fetch(&http, &issuer) {
                Ok(idp) => idp,
                Err(e) if pref == AuthPreference::Auto => {
                    let reason = BasicReason::Discovery(e.to_string());
                    return Err(unavailable(BasicReason::external(issuer, reason)));
                }
                Err(e) => return Err(e),
            };
            match login::external_plan(&idp, issuer, client_id, scope, resource, pref)? {
                LoginStep::Device(plan) => plan,
                LoginStep::Basic { reason } => return Err(unavailable(reason)),
                LoginStep::External { .. } => return Err(AuthError::NoDeviceEndpoint.into()),
            }
        }
        LoginStep::Basic { reason } => return Err(unavailable(reason)),
    };
    let mut flow = plan.begin(&http, store.as_ref())?;
    let mut err = io::stderr().lock();
    let grant = &flow.grant;
    writeln!(err, "To sign in, open {}", grant.verification_uri)?;
    writeln!(err, "and enter the code {}", grant.user_code)?;
    if let Some(complete) = &grant.verification_uri_complete {
        writeln!(err, "(or open {complete})")?;
    }
    writeln!(err, "Waiting for approval...")?;
    let tokens = loop {
        thread::sleep(flow.interval());
        match flow.poll(&http)? {
            Poll::Pending | Poll::SlowDown => {}
            Poll::Granted(tokens) => break tokens,
            Poll::Denied => return Err(AuthError::AccessDenied.into()),
            Poll::Expired => return Err(AuthError::Expired.into()),
        }
    };
    let mut stored = plan.session(url, "", flow.client_id(), tokens, SystemTime::now());
    login::resolve_account(&mut stored, url, tls)?;
    if let Some(store) = &store {
        store.save(&stored)?;
    }
    writeln!(err, "Signed in as {}.", stored.account)?;
    Credential::oauth(TokenSession { stored, store })
}

fn unavailable(reason: BasicReason) -> Error {
    Error::DeviceUnavailable(Box::new(reason))
}
