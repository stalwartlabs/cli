/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::AuthError;
use crate::auth::store::StoredSession;
use crate::error::{Error, Result};
use crate::jmap::http::HttpClient;
use reqwest::StatusCode;
use reqwest::blocking::Response;
use serde::Deserialize;
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthCode {
    AuthorizationPending,
    SlowDown,
    AccessDenied,
    ExpiredToken,
    InvalidGrant,
    InvalidClient,
    UnauthorizedClient,
    InvalidScope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenSet {
    pub access: String,
    pub refresh: Option<String>,
    pub expires_at: Option<SystemTime>,
    pub scope: Option<String>,
    pub username: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OAuthErrorBody {
    pub error: String,
    #[serde(default)]
    pub error_description: Option<String>,
}

pub enum TokenReply {
    Granted(TokenSet),
    Refused {
        status: StatusCode,
        body: OAuthErrorBody,
    },
}

impl TokenReply {
    pub fn read(response: Response, now: SystemTime) -> Result<Self> {
        let status = response.status();
        if status.is_success() {
            let body: TokenResponse = response.json()?;
            return Ok(TokenReply::Granted(TokenSet {
                access: body.access_token,
                refresh: body.refresh_token,
                expires_at: body
                    .expires_in
                    .and_then(|secs| now.checked_add(Duration::from_secs(secs))),
                scope: body.scope,
                username: body.username,
            }));
        }
        match response.json::<OAuthErrorBody>() {
            Ok(body) => Ok(TokenReply::Refused { status, body }),
            Err(_) => Err(status.into()),
        }
    }
}

impl OAuthErrorBody {
    pub fn code(&self) -> Option<OAuthCode> {
        hashify::map!(
            self.error.as_bytes(),
            OAuthCode,
            "authorization_pending" => OAuthCode::AuthorizationPending,
            "slow_down" => OAuthCode::SlowDown,
            "access_denied" => OAuthCode::AccessDenied,
            "expired_token" => OAuthCode::ExpiredToken,
            "invalid_grant" => OAuthCode::InvalidGrant,
            "invalid_client" => OAuthCode::InvalidClient,
            "unauthorized_client" => OAuthCode::UnauthorizedClient,
            "invalid_scope" => OAuthCode::InvalidScope,
        )
        .copied()
    }

    pub fn into_error(self) -> AuthError {
        match self.code() {
            Some(OAuthCode::UnauthorizedClient) => AuthError::UnauthorizedClient,
            Some(OAuthCode::InvalidClient) => AuthError::InvalidClient,
            Some(OAuthCode::InvalidScope) => AuthError::InvalidScope,
            Some(OAuthCode::AccessDenied) => AuthError::AccessDenied,
            Some(OAuthCode::ExpiredToken) => AuthError::Expired,
            _ => AuthError::OAuth {
                error: self.error,
                description: self.error_description,
            },
        }
    }
}

pub fn refresh(
    http: &HttpClient,
    endpoint: &str,
    client_id: &str,
    refresh_token: &str,
    resource: Option<&str>,
) -> Result<TokenSet> {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    if let Some(resource) = resource {
        form.push(("resource", resource));
    }
    let response = http.post_form_anonymous(endpoint, &form)?;
    match TokenReply::read(response, SystemTime::now())? {
        TokenReply::Granted(tokens) => Ok(tokens),
        TokenReply::Refused { status, .. }
            if status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED =>
        {
            Err(Error::SessionExpired)
        }
        TokenReply::Refused { body, .. } => Err(body.into_error().into()),
    }
}

pub fn refresh_session(http: &HttpClient, stored: &StoredSession) -> Result<StoredSession> {
    let Some(refresh_token) = stored.refresh_token.as_deref() else {
        return Err(Error::SessionExpired);
    };
    let tokens = refresh(
        http,
        &stored.token_endpoint,
        &stored.client_id,
        refresh_token,
        stored.resource.as_deref(),
    )?;
    Ok(stored.with_tokens(tokens, SystemTime::now()))
}

pub fn revoke(http: &HttpClient, endpoint: &str, client_id: &str, token: &str) -> Result<()> {
    let form = [("token", token), ("client_id", client_id)];
    let status = http.post_form_anonymous(endpoint, &form)?.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(status.into())
    }
}
