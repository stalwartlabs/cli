/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::store::{StoreWarning, StoredSession, TokenStore};
use crate::auth::token;
use crate::error::{Error, Result};
use crate::jmap::auth::Auth;
use crate::jmap::http::HttpClient;
use reqwest::header::HeaderValue;
use std::sync::{Mutex, PoisonError, RwLock};
use std::time::{Duration, SystemTime};

const REFRESH_MARGIN: Duration = Duration::from_secs(60);

pub enum Credential {
    Static(HeaderValue),
    OAuth(Box<OAuthCredential>),
}

pub struct OAuthCredential {
    header: RwLock<HeaderState>,
    refresh: Mutex<()>,
    store: Option<TokenStore>,
}

pub struct TokenSession {
    pub stored: StoredSession,
    pub store: Option<TokenStore>,
}

struct HeaderState {
    value: HeaderValue,
    generation: u64,
    refresh_at: Option<SystemTime>,
    session: StoredSession,
}

impl Credential {
    pub fn from_auth(auth: &Auth) -> Result<Self> {
        auth.header().map(Credential::Static)
    }

    pub fn oauth(session: TokenSession) -> Result<Self> {
        let TokenSession { stored, store } = session;
        Ok(Credential::OAuth(Box::new(OAuthCredential {
            header: RwLock::new(HeaderState::of(stored, 0)?),
            refresh: Mutex::new(()),
            store,
        })))
    }

    pub fn header(&self) -> (HeaderValue, u64) {
        match self {
            Credential::Static(value) => (value.clone(), 0),
            Credential::OAuth(oauth) => {
                let state = oauth.header.read().unwrap_or_else(PoisonError::into_inner);
                (state.value.clone(), state.generation)
            }
        }
    }

    pub fn needs_refresh(&self, now: SystemTime) -> bool {
        match self {
            Credential::Static(_) => false,
            Credential::OAuth(oauth) => oauth
                .header
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .refresh_at
                .is_some_and(|at| now >= at),
        }
    }

    pub fn is_refreshable(&self) -> bool {
        matches!(self, Credential::OAuth(_))
    }

    pub fn is_unexpired(&self, now: SystemTime) -> bool {
        match self {
            Credential::Static(_) => true,
            Credential::OAuth(oauth) => oauth
                .header
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .session
                .expires_at()
                .is_none_or(|at| now < at),
        }
    }

    pub fn take_store_warnings(&self) -> impl Iterator<Item = StoreWarning> {
        let store = match self {
            Credential::OAuth(oauth) => oauth.store.as_ref(),
            Credential::Static(_) => None,
        };
        store.into_iter().flat_map(TokenStore::take_warnings)
    }

    pub fn refresh_after(&self, http: &HttpClient, seen_generation: u64) -> Result<()> {
        let Credential::OAuth(oauth) = self else {
            return Err(Error::AuthFailed);
        };
        let guard = oauth.refresh.lock().unwrap_or_else(PoisonError::into_inner);
        let (current, held) = {
            let state = oauth.header.read().unwrap_or_else(PoisonError::into_inner);
            if state.generation != seen_generation {
                return Ok(());
            }
            (state.generation, state.session.clone())
        };
        let refreshed = match &oauth.store {
            Some(store) => store.refresh(&held, |latest| token::refresh_session(http, latest))?,
            None => token::refresh_session(http, &held)?,
        };
        let state = HeaderState::of(refreshed, current.wrapping_add(1))?;
        *oauth.header.write().unwrap_or_else(PoisonError::into_inner) = state;
        drop(guard);
        Ok(())
    }

    pub fn session(&self) -> Option<StoredSession> {
        match self {
            Credential::Static(_) => None,
            Credential::OAuth(oauth) => Some(
                oauth
                    .header
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .session
                    .clone(),
            ),
        }
    }
}

impl HeaderState {
    fn of(session: StoredSession, generation: u64) -> Result<Self> {
        Ok(HeaderState {
            value: Auth::bearer_header(&session.access_token)?,
            generation,
            refresh_at: session.refresh_token.as_ref().and(session.refresh_at()),
            session,
        })
    }
}

impl StoredSession {
    pub fn refresh_at(&self) -> Option<SystemTime> {
        let expires_at = self.expires_at()?;
        let lifetime = expires_at
            .duration_since(self.obtained_at())
            .unwrap_or_default();
        let margin = REFRESH_MARGIN.min(lifetime / 2);
        expires_at.checked_sub(margin)
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/credential/tests.rs"]
mod tests;
