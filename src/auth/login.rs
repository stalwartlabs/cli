/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::device::DeviceFlow;
use crate::auth::discover::{DeviceEndpoints, Discovery, DiscoveryKind, Metadata};
use crate::auth::register::{CLIENT_NAME, register_device_client};
use crate::auth::store::{CachedClient, LoginMethod, StoredSession, TokenStore, unix_secs};
use crate::auth::token::TokenSet;
use crate::auth::{AuthError, FULL_SCOPE, STALWART_SCOPE};
use crate::error::{Error, Result};
use crate::jmap::auth::Auth;
use crate::jmap::cache::normalise_url;
use crate::jmap::credential::Credential;
use crate::jmap::http::{ConnectOptions, HttpClient, TlsVerify};
use crate::jmap::session::Session;
use std::mem;
use std::sync::Arc;
use std::time::SystemTime;

const OFFLINE_ACCESS: &str = "offline_access";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthPreference {
    Auto,
    Device,
    Basic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BasicReason {
    Preferred,
    Recovery,
    NoDeviceSupport,
    NoClientId,
    DcrRefused(String),
    Discovery(String),
    External {
        issuer: String,
        reason: Box<BasicReason>,
    },
}

impl BasicReason {
    pub fn external(issuer: impl Into<String>, reason: BasicReason) -> Self {
        BasicReason::External {
            issuer: issuer.into(),
            reason: Box::new(reason),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientSource {
    Configured(String),
    Cached {
        client_id: String,
        registration_endpoint: String,
    },
    Dynamic {
        registration_endpoint: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevicePlan {
    pub method: LoginMethod,
    pub issuer: Option<String>,
    pub endpoints: DeviceEndpoints,
    pub client: ClientSource,
    pub scope: String,
    pub resource: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginStep {
    Device(DevicePlan),
    External {
        issuer: String,
        client_id: String,
        scope: String,
        resource: Option<String>,
    },
    Basic {
        reason: BasicReason,
    },
}

pub fn stored_session(
    store: Option<&TokenStore>,
    account: Option<&str>,
    pref: AuthPreference,
    now: SystemTime,
) -> Option<StoredSession> {
    if pref == AuthPreference::Basic {
        return None;
    }
    let store = store?;
    let session = match account {
        Some(account) => store.load(account),
        None => store.last(),
    }?;
    session.is_usable(now).then_some(session)
}

pub fn plan(
    server_url: &str,
    discovery: Result<Discovery>,
    pref: AuthPreference,
    client_id: Option<&str>,
    cached_client: impl FnOnce(&str) -> Option<String>,
) -> Result<LoginStep> {
    let basic = |reason: BasicReason, error: Error| match pref {
        AuthPreference::Device => Err(error),
        _ => Ok(LoginStep::Basic { reason }),
    };
    if pref == AuthPreference::Basic {
        return Ok(LoginStep::Basic {
            reason: BasicReason::Preferred,
        });
    }
    let discovery = match discovery {
        Ok(discovery) => discovery,
        Err(e) => return basic(BasicReason::Discovery(e.to_string()), e),
    };
    let metadata = &discovery.metadata;
    match discovery.kind() {
        DiscoveryKind::Internal if discovery.is_recovery() => {
            basic(BasicReason::Recovery, AuthError::NoDeviceEndpoint.into())
        }
        DiscoveryKind::Internal => {
            let Some(endpoints) = metadata.device_endpoints() else {
                return basic(
                    BasicReason::NoDeviceSupport,
                    AuthError::NoDeviceEndpoint.into(),
                );
            };
            let client = match (client_id, &metadata.registration_endpoint) {
                (Some(id), _) => ClientSource::Configured(id.to_string()),
                (None, Some(endpoint)) => match cached_client(endpoint) {
                    Some(client_id) => ClientSource::Cached {
                        client_id,
                        registration_endpoint: endpoint.clone(),
                    },
                    None => ClientSource::Dynamic {
                        registration_endpoint: endpoint.clone(),
                    },
                },
                (None, None) => {
                    return basic(BasicReason::NoClientId, AuthError::MissingClientId.into());
                }
            };
            Ok(LoginStep::Device(DevicePlan {
                method: LoginMethod::Device,
                issuer: None,
                endpoints,
                client,
                scope: STALWART_SCOPE.to_string(),
                resource: None,
            }))
        }
        DiscoveryKind::External { issuer } => {
            let Some(client_id) = client_id else {
                return basic(
                    BasicReason::external(issuer, BasicReason::NoClientId),
                    AuthError::MissingClientId.into(),
                );
            };
            Ok(LoginStep::External {
                issuer: issuer.to_string(),
                client_id: client_id.to_string(),
                scope: external_scope(metadata),
                resource: external_resource(metadata, server_url),
            })
        }
    }
}

pub fn external_plan(
    idp: &Metadata,
    issuer: String,
    client_id: String,
    scope: String,
    resource: Option<String>,
    pref: AuthPreference,
) -> Result<LoginStep> {
    match idp.device_endpoints() {
        Some(endpoints) => Ok(LoginStep::Device(DevicePlan {
            method: LoginMethod::ExternalDevice,
            issuer: Some(issuer),
            endpoints,
            client: ClientSource::Configured(client_id),
            scope,
            resource,
        })),
        None if pref == AuthPreference::Device => Err(AuthError::NoDeviceEndpoint.into()),
        None => Ok(LoginStep::Basic {
            reason: BasicReason::external(issuer, BasicReason::NoDeviceSupport),
        }),
    }
}

fn external_scope(metadata: &Metadata) -> String {
    let mut scope = metadata.stalwart_required_scopes.join(" ");
    for extra in [OFFLINE_ACCESS, FULL_SCOPE] {
        if metadata.supports_scope(extra)
            && !metadata.stalwart_required_scopes.iter().any(|s| s == extra)
        {
            if !scope.is_empty() {
                scope.push(' ');
            }
            scope.push_str(extra);
        }
    }
    scope
}

fn external_resource(metadata: &Metadata, server_url: &str) -> Option<String> {
    let origin = normalise_url(server_url);
    metadata
        .protected_resources
        .iter()
        .any(|r| normalise_url(r) == origin)
        .then_some(origin)
}

impl DevicePlan {
    pub fn client_id(&self, http: &HttpClient, store: Option<&TokenStore>) -> Result<String> {
        match &self.client {
            ClientSource::Configured(client_id) | ClientSource::Cached { client_id, .. } => {
                Ok(client_id.clone())
            }
            ClientSource::Dynamic {
                registration_endpoint,
            } => {
                let client_id = register_device_client(http, registration_endpoint, CLIENT_NAME)?;
                if let Some(store) = store {
                    store.save_client(&CachedClient {
                        registration_endpoint: registration_endpoint.clone(),
                        client_id: client_id.clone(),
                    })?;
                }
                Ok(client_id)
            }
        }
    }

    pub fn begin(&mut self, http: &HttpClient, store: Option<&TokenStore>) -> Result<DeviceFlow> {
        let client_id = self.client_id(http, store)?;
        match self.start(http, &client_id) {
            Err(Error::Auth(AuthError::InvalidClient)) => {
                let ClientSource::Cached {
                    registration_endpoint,
                    ..
                } = &mut self.client
                else {
                    return Err(AuthError::InvalidClient.into());
                };
                if let Some(store) = store {
                    store.forget_client()?;
                }
                self.client = ClientSource::Dynamic {
                    registration_endpoint: mem::take(registration_endpoint),
                };
                let client_id = self.client_id(http, store)?;
                self.start(http, &client_id)
            }
            started => started,
        }
    }

    pub fn start(&self, http: &HttpClient, client_id: &str) -> Result<DeviceFlow> {
        DeviceFlow::start(
            http,
            self.endpoints.clone(),
            client_id,
            &self.scope,
            self.resource.as_deref(),
        )
    }

    pub fn session(
        &self,
        server_url: &str,
        account: &str,
        client_id: &str,
        tokens: TokenSet,
        now: SystemTime,
    ) -> StoredSession {
        let account = tokens
            .username
            .as_deref()
            .filter(|_| account.is_empty())
            .unwrap_or(account)
            .to_string();
        StoredSession {
            server: normalise_url(server_url),
            account,
            method: self.method,
            issuer: self.issuer.clone(),
            token_endpoint: self.endpoints.token.clone(),
            revocation_endpoint: self.endpoints.revocation.clone(),
            client_id: client_id.to_string(),
            scope: tokens.scope.unwrap_or_else(|| self.scope.clone()),
            resource: self.resource.clone(),
            access_token: tokens.access,
            refresh_token: tokens.refresh,
            expires_at: tokens.expires_at.map(unix_secs),
            obtained_at: unix_secs(now),
        }
    }
}

pub fn resolve_account(session: &mut StoredSession, url: &str, tls: TlsVerify) -> Result<()> {
    if !session.account.is_empty() {
        return Ok(());
    }
    let credential = Credential::Static(Auth::bearer_header(&session.access_token)?);
    let http = HttpClient::new(&ConnectOptions {
        url: url.to_string(),
        credential: Arc::new(credential),
        tls,
    })?;
    let username = Session::fetch(&http)?.username;
    if username.trim().is_empty() {
        return Err(Error::UnexpectedResponse(
            "the server did not name the signed-in account".into(),
        ));
    }
    session.account = username.into();
    Ok(())
}

pub fn is_dcr_refusal(error: &Error) -> bool {
    matches!(
        error,
        Error::Auth(AuthError::RegistrationRefused(_) | AuthError::UnauthorizedClient)
    )
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/auth/login/tests.rs"]
mod tests;
