/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::DEVICE_CODE_GRANT;
use crate::auth::discover::DeviceEndpoints;
use crate::auth::token::{OAuthCode, TokenReply, TokenSet};
use crate::error::{Error, Result};
use crate::jmap::http::HttpClient;
use reqwest::StatusCode;
use serde::Deserialize;
use std::time::{Duration, SystemTime};

const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);
const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);
const MIN_INTERVAL: Duration = Duration::from_secs(1);
const MAX_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceGrant {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_at: SystemTime,
}

#[derive(Debug)]
pub struct DeviceFlow {
    pub grant: DeviceGrant,
    endpoints: DeviceEndpoints,
    client_id: String,
    device_code: String,
    resource: Option<String>,
    interval: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    Pending,
    SlowDown,
    Granted(TokenSet),
    Denied,
    Expired,
}

#[derive(Deserialize)]
struct DeviceResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in: u64,
    #[serde(default)]
    interval: Option<u64>,
}

impl DeviceFlow {
    pub fn start(
        http: &HttpClient,
        endpoints: DeviceEndpoints,
        client_id: &str,
        scope: &str,
        resource: Option<&str>,
    ) -> Result<Self> {
        let mut form = vec![("client_id", client_id), ("scope", scope)];
        if let Some(resource) = resource {
            form.push(("resource", resource));
        }
        let response = http.post_form_anonymous(&endpoints.device, &form)?;
        if !response.status().is_success() {
            return match TokenReply::read(response, SystemTime::now())? {
                TokenReply::Refused { body, .. } => Err(body.into_error().into()),
                TokenReply::Granted(_) => Err(Error::UnexpectedResponse(
                    "device authorization returned a token".into(),
                )),
            };
        }
        let body: DeviceResponse = response.json()?;
        let expires_at = SystemTime::now()
            .checked_add(Duration::from_secs(body.expires_in))
            .ok_or_else(|| {
                Error::UnexpectedResponse("device code expires_in out of range".into())
            })?;
        Ok(DeviceFlow {
            grant: DeviceGrant {
                user_code: body.user_code,
                verification_uri: http.resolve(&body.verification_uri),
                verification_uri_complete: body
                    .verification_uri_complete
                    .map(|uri| http.resolve(&uri)),
                expires_at,
            },
            endpoints,
            client_id: client_id.to_string(),
            device_code: body.device_code,
            resource: resource.map(str::to_string),
            interval: body
                .interval
                .map_or(DEFAULT_INTERVAL, Duration::from_secs)
                .clamp(MIN_INTERVAL, MAX_INTERVAL),
        })
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn poll(&mut self, http: &HttpClient) -> Result<Poll> {
        let now = SystemTime::now();
        if now >= self.grant.expires_at {
            return Ok(Poll::Expired);
        }
        let mut form = vec![
            ("grant_type", DEVICE_CODE_GRANT),
            ("device_code", self.device_code.as_str()),
            ("client_id", self.client_id.as_str()),
        ];
        if let Some(resource) = &self.resource {
            form.push(("resource", resource));
        }
        let response = match http.post_form_anonymous(&self.endpoints.token, &form) {
            Ok(response) => response,
            Err(Error::Network(_)) => return Ok(self.back_off()),
            Err(e) => return Err(e),
        };
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Ok(self.back_off());
        }
        match TokenReply::read(response, now)? {
            TokenReply::Granted(tokens) => Ok(Poll::Granted(tokens)),
            TokenReply::Refused { body, .. } => match body.code() {
                Some(OAuthCode::AuthorizationPending) => Ok(Poll::Pending),
                Some(OAuthCode::SlowDown) => {
                    self.interval = self
                        .interval
                        .saturating_add(SLOW_DOWN_STEP)
                        .min(MAX_INTERVAL);
                    Ok(Poll::SlowDown)
                }
                Some(OAuthCode::AccessDenied) => Ok(Poll::Denied),
                Some(OAuthCode::ExpiredToken) => Ok(Poll::Expired),
                _ => Err(body.into_error().into()),
            },
        }
    }

    fn back_off(&mut self) -> Poll {
        self.interval = self.interval.saturating_mul(2).min(MAX_INTERVAL);
        Poll::SlowDown
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/auth/device/tests.rs"]
mod tests;
