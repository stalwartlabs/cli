/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::AuthError;
use crate::auth::device::{DeviceFlow, Poll};
use crate::auth::discover::{Discovery, IdpMetadata};
use crate::auth::login::{self, AuthPreference, BasicReason, DevicePlan, LoginStep};
use crate::auth::store::{StoredSession, TokenStore};
use crate::auth::token;
use crate::error::{Error, Result};
use crate::jmap::http::{HttpClient, TlsVerify};
use crate::tui::exec::{Cancel, JobCx};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime};

const CANCEL_POLL: Duration = Duration::from_millis(100);

#[derive(Clone)]
pub struct AuthTarget {
    pub url: String,
    pub tls: TlsVerify,
    pub auth_dir: Option<PathBuf>,
    pub writer_id: u32,
}

pub enum AuthJob {
    Stored {
        target: AuthTarget,
        account: Option<String>,
    },
    Plan {
        target: AuthTarget,
        account: Option<String>,
        pref: AuthPreference,
        client_id: Option<String>,
    },
    DeviceStart {
        target: AuthTarget,
        plan: Box<DevicePlan>,
    },
    DeviceWait {
        target: AuthTarget,
        plan: Box<DevicePlan>,
        flow: Box<DeviceFlow>,
        account: String,
        cancel: Cancel,
    },
    Revoke {
        target: AuthTarget,
        session: Box<StoredSession>,
    },
}

pub enum Planned {
    Nothing,
    Stored(Box<StoredSession>),
    Device(Box<DevicePlan>),
    Basic(BasicReason),
}

pub enum AuthOutput {
    Planned(Result<Planned, Box<Error>>),
    DeviceStarted(Result<(Box<DevicePlan>, Box<DeviceFlow>), Box<Error>>),
    DeviceDone(Result<Option<Box<StoredSession>>, Box<Error>>),
    Revoked(Result<(), Box<Error>>),
}

impl AuthOutput {
    pub fn error(&self) -> Option<&Error> {
        match self {
            AuthOutput::Planned(result) => result.as_ref().err(),
            AuthOutput::DeviceStarted(result) => result.as_ref().err(),
            AuthOutput::DeviceDone(result) => result.as_ref().err(),
            AuthOutput::Revoked(result) => result.as_ref().err(),
        }
        .map(AsRef::as_ref)
    }
}

impl AuthTarget {
    pub fn store(&self) -> Option<TokenStore> {
        TokenStore::new(self.auth_dir.as_deref()?, &self.url, self.writer_id).ok()
    }

    fn http(&self) -> Result<HttpClient> {
        HttpClient::anonymous(&self.url, self.tls)
    }
}

impl AuthJob {
    pub fn run(self, _cx: &JobCx) -> AuthOutput {
        match self {
            AuthJob::Stored { target, account } => AuthOutput::Planned(Ok(
                match stored(&target, account.as_deref(), AuthPreference::Auto) {
                    Some(session) => Planned::Stored(Box::new(session)),
                    None => Planned::Nothing,
                },
            )),
            AuthJob::Plan {
                target,
                account,
                pref,
                client_id,
            } => AuthOutput::Planned(
                plan(&target, account.as_deref(), pref, client_id.as_deref()).map_err(Box::new),
            ),
            AuthJob::DeviceStart { target, mut plan } => AuthOutput::DeviceStarted(
                target
                    .http()
                    .and_then(|http| plan.begin(&http, target.store().as_ref()))
                    .map(|flow| (plan, Box::new(flow)))
                    .map_err(Box::new),
            ),
            AuthJob::DeviceWait {
                target,
                plan,
                flow,
                account,
                cancel,
            } => AuthOutput::DeviceDone(
                wait(&target, &plan, *flow, &account, &cancel)
                    .map(|s| s.map(Box::new))
                    .map_err(Box::new),
            ),
            AuthJob::Revoke { target, session } => {
                AuthOutput::Revoked(revoke(&target, &session).map_err(Box::new))
            }
        }
    }
}

fn stored(
    target: &AuthTarget,
    account: Option<&str>,
    pref: AuthPreference,
) -> Option<StoredSession> {
    let store = target.store();
    login::stored_session(store.as_ref(), account, pref, SystemTime::now())
}

fn plan(
    target: &AuthTarget,
    account: Option<&str>,
    pref: AuthPreference,
    client_id: Option<&str>,
) -> Result<Planned> {
    if let Some(session) = stored(target, account, pref) {
        return Ok(Planned::Stored(Box::new(session)));
    }
    let store = target.store();
    let http = target.http()?;
    let step = login::plan(
        &target.url,
        Discovery::fetch(&http, account),
        pref,
        client_id,
        |endpoint| store.as_ref()?.cached_client(endpoint),
    )?;
    let step = match step {
        LoginStep::External {
            issuer,
            client_id,
            scope,
            resource,
        } => match IdpMetadata::fetch(&http, &issuer) {
            Ok(idp) => login::external_plan(&idp, issuer, client_id, scope, resource, pref)?,
            Err(e) if pref == AuthPreference::Device => return Err(e),
            Err(e) => LoginStep::Basic {
                reason: BasicReason::external(issuer, BasicReason::Discovery(e.to_string())),
            },
        },
        step => step,
    };
    Ok(match step {
        LoginStep::Device(plan) => Planned::Device(Box::new(plan)),
        LoginStep::Basic { reason } => Planned::Basic(reason),
        LoginStep::External { .. } => Planned::Basic(BasicReason::NoDeviceSupport),
    })
}

fn wait(
    target: &AuthTarget,
    plan: &DevicePlan,
    mut flow: DeviceFlow,
    account: &str,
    cancel: &Cancel,
) -> Result<Option<StoredSession>> {
    let http = target.http()?;
    loop {
        if !sleep(flow.interval(), cancel) {
            return Ok(None);
        }
        if SystemTime::now() >= flow.grant.expires_at {
            return Err(AuthError::Expired.into());
        }
        match flow.poll(&http)? {
            Poll::Pending | Poll::SlowDown => {}
            Poll::Granted(tokens) => {
                let mut session = plan.session(
                    &target.url,
                    account,
                    flow.client_id(),
                    tokens,
                    SystemTime::now(),
                );
                login::resolve_account(&mut session, &target.url, target.tls)?;
                if let Some(store) = target.store() {
                    store.save(&session)?;
                }
                return Ok(Some(session));
            }
            Poll::Denied => return Err(AuthError::AccessDenied.into()),
            Poll::Expired => return Err(AuthError::Expired.into()),
        }
    }
}

fn sleep(total: Duration, cancel: &Cancel) -> bool {
    let mut left = total;
    while !left.is_zero() {
        if cancel.is_cancelled() {
            return false;
        }
        let step = left.min(CANCEL_POLL);
        thread::sleep(step);
        left -= step;
    }
    !cancel.is_cancelled()
}

fn revoke(target: &AuthTarget, session: &StoredSession) -> Result<()> {
    match target.store() {
        Some(store) => store.sign_out(&session.account, |latest| {
            revoke_token(target, latest.unwrap_or(session))
        })?,
        None => revoke_token(target, session),
    }
}

fn revoke_token(target: &AuthTarget, session: &StoredSession) -> Result<()> {
    let Some(endpoint) = &session.revocation_endpoint else {
        return Ok(());
    };
    let token = session
        .refresh_token
        .as_deref()
        .unwrap_or(&session.access_token);
    target
        .http()
        .and_then(|http| token::revoke(&http, endpoint, &session.client_id, token))
}
