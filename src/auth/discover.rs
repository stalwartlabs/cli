/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::{AuthError, DEVICE_CODE_GRANT};
use crate::error::Result;
use crate::jmap::http::{HttpClient, is_absolute};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

const DISCOVER_PATH: &str = "/api/discover/";
const AUTH_SERVER_WELL_KNOWN: &str = "/.well-known/oauth-authorization-server";
const OPENID_WELL_KNOWN: &str = "/.well-known/openid-configuration";

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Metadata {
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub token_endpoint: Option<String>,
    #[serde(default)]
    pub device_authorization_endpoint: Option<String>,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
    #[serde(default)]
    pub grant_types_supported: Option<Vec<String>>,
    #[serde(default)]
    pub stalwart_required_scopes: Vec<String>,
    #[serde(default)]
    pub protected_resources: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    pub metadata: Metadata,
    pub form: EndpointForm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointForm {
    Absolute,
    Relative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoveryKind<'a> {
    Internal,
    External { issuer: &'a str },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEndpoints {
    pub device: String,
    pub token: String,
    pub revocation: Option<String>,
}

impl Discovery {
    pub fn fetch(http: &HttpClient, account: Option<&str>) -> Result<Self> {
        if let Some(account) = account.map(str::trim).filter(|a| !a.is_empty()) {
            let mut path = String::with_capacity(DISCOVER_PATH.len() + account.len() * 3);
            path.push_str(DISCOVER_PATH);
            path.extend(utf8_percent_encode(account, NON_ALPHANUMERIC));
            if let Ok(Some(metadata)) = Metadata::fetch(http, &http.resolve(&path)) {
                return Ok(Discovery::new(metadata, http));
            }
        }
        match Metadata::fetch(http, &http.resolve(AUTH_SERVER_WELL_KNOWN))? {
            Some(metadata) => Ok(Discovery::new(metadata, http)),
            None => Err(AuthError::DiscoveryFailed(404).into()),
        }
    }

    fn new(metadata: Metadata, http: &HttpClient) -> Self {
        let form = match &metadata.device_authorization_endpoint {
            Some(endpoint) if !is_absolute(endpoint) => EndpointForm::Relative,
            _ => EndpointForm::Absolute,
        };
        Discovery {
            metadata: metadata.resolved(http),
            form,
        }
    }

    pub fn is_recovery(&self) -> bool {
        self.form == EndpointForm::Relative
            && self
                .metadata
                .issuer
                .as_deref()
                .is_some_and(|issuer| !issuer.is_empty())
    }

    pub fn kind(&self) -> DiscoveryKind<'_> {
        match (
            &self.metadata.device_authorization_endpoint,
            &self.metadata.issuer,
        ) {
            (None, Some(issuer)) if !issuer.is_empty() => DiscoveryKind::External { issuer },
            _ => DiscoveryKind::Internal,
        }
    }
}

impl Metadata {
    fn fetch(http: &HttpClient, url: &str) -> Result<Option<Self>> {
        let response = http.get_anonymous(url)?;
        let status = response.status();
        if status.is_success() {
            Ok(Some(response.json()?))
        } else if status.is_client_error() {
            Ok(None)
        } else {
            Err(AuthError::DiscoveryFailed(status.as_u16()).into())
        }
    }

    fn resolved(mut self, http: &HttpClient) -> Self {
        for endpoint in [
            &mut self.token_endpoint,
            &mut self.device_authorization_endpoint,
            &mut self.registration_endpoint,
            &mut self.revocation_endpoint,
        ]
        .into_iter()
        .flatten()
        {
            *endpoint = http.resolve(endpoint);
        }
        self
    }

    pub fn device_endpoints(&self) -> Option<DeviceEndpoints> {
        let grants_device = self
            .grant_types_supported
            .as_ref()
            .is_none_or(|grants| grants.iter().any(|g| g == DEVICE_CODE_GRANT));
        Some(DeviceEndpoints {
            device: self
                .device_authorization_endpoint
                .clone()
                .filter(|_| grants_device)?,
            token: self.token_endpoint.clone()?,
            revocation: self.revocation_endpoint.clone(),
        })
    }

    pub fn supports_scope(&self, scope: &str) -> bool {
        self.scopes_supported.iter().any(|s| s == scope)
    }
}

pub struct IdpMetadata;

impl IdpMetadata {
    pub fn fetch(http: &HttpClient, issuer: &str) -> Result<Metadata> {
        let mut last_status = 404;
        for url in [insertion_url(issuer), appended_url(issuer)] {
            let response = http.get_anonymous(&url)?;
            let status = response.status();
            if !status.is_success() {
                last_status = status.as_u16();
                continue;
            }
            let metadata: Metadata = response.json()?;
            if metadata.issuer.as_deref() != Some(issuer) {
                return Err(AuthError::IssuerMismatch {
                    expected: issuer.to_string(),
                    got: metadata.issuer.unwrap_or_default(),
                }
                .into());
            }
            return Ok(metadata);
        }
        Err(AuthError::DiscoveryFailed(last_status).into())
    }
}

fn insertion_url(issuer: &str) -> String {
    let trimmed = issuer.trim_end_matches('/');
    let (scheme, rest) = trimmed.split_once("://").unwrap_or(("https", trimmed));
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut url = String::with_capacity(issuer.len() + AUTH_SERVER_WELL_KNOWN.len() + 4);
    url.push_str(scheme);
    url.push_str("://");
    url.push_str(host);
    url.push_str(AUTH_SERVER_WELL_KNOWN);
    if !path.is_empty() {
        url.push('/');
        url.push_str(path);
    }
    url
}

fn appended_url(issuer: &str) -> String {
    let trimmed = issuer.trim_end_matches('/');
    let mut url = String::with_capacity(trimmed.len() + OPENID_WELL_KNOWN.len());
    url.push_str(trimmed);
    url.push_str(OPENID_WELL_KNOWN);
    url
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/auth/discover/tests.rs"]
mod tests;
