/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::token::TokenSet;
use crate::error::{Error, Result};
use crate::jmap::cache::{hash_b64, normalise_url, tmp_path};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LAST: &str = "last";
const CLIENT: &str = "client.json";
const SESSION_SUFFIX: &str = ".json";
const LOCK_SUFFIX: &str = ".lock";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoginMethod {
    Device,
    ExternalDevice,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSession {
    pub server: String,
    pub account: String,
    pub method: LoginMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    pub token_endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_endpoint: Option<String>,
    pub client_id: String,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    pub obtained_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedClient {
    pub registration_endpoint: String,
    pub client_id: String,
}

#[derive(Debug)]
pub struct TokenStore {
    dir: PathBuf,
    writer_id: u32,
    counter: AtomicU64,
    pending: AtomicU8,
    reported: AtomicU8,
}

#[derive(Clone, Copy)]
enum LockMode {
    Shared,
    Exclusive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreWarning {
    Unlocked,
    NotSaved,
}

impl StoreWarning {
    const ALL: [StoreWarning; 2] = [StoreWarning::Unlocked, StoreWarning::NotSaved];

    const fn bit(self) -> u8 {
        match self {
            StoreWarning::Unlocked => 1,
            StoreWarning::NotSaved => 2,
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            StoreWarning::Unlocked => {
                "the token store cannot be locked on this filesystem; concurrent sign-ins may conflict"
            }
            StoreWarning::NotSaved => {
                "the refreshed session could not be saved; the next run may have to sign in again"
            }
        }
    }
}

impl StoredSession {
    pub fn expires_at(&self) -> Option<SystemTime> {
        self.expires_at
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
    }

    pub fn obtained_at(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(self.obtained_at)
    }

    pub fn is_usable(&self, now: SystemTime) -> bool {
        self.refresh_token.is_some() || self.expires_at().is_none_or(|at| now < at)
    }

    pub fn with_tokens(&self, tokens: TokenSet, now: SystemTime) -> Self {
        StoredSession {
            access_token: tokens.access,
            refresh_token: tokens.refresh.or_else(|| self.refresh_token.clone()),
            expires_at: tokens.expires_at.map(unix_secs),
            obtained_at: unix_secs(now),
            ..self.clone()
        }
    }
}

pub fn unix_secs(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl TokenStore {
    pub fn new(base: &Path, server_url: &str, writer_id: u32) -> Result<Self> {
        let dir = base.join(hash_b64(normalise_url(server_url).as_bytes()));
        fs::create_dir_all(&dir)?;
        restrict_dir(&dir)?;
        Ok(TokenStore {
            dir,
            writer_id,
            counter: AtomicU64::new(0),
            pending: AtomicU8::new(0),
            reported: AtomicU8::new(0),
        })
    }

    pub fn load(&self, account: &str) -> Option<StoredSession> {
        let _lock = self.lock(account, LockMode::Shared);
        read_json(&self.session_path(account))
    }

    pub fn last_account(&self) -> Option<String> {
        let name = fs::read_to_string(self.dir.join(LAST)).ok()?;
        let name = name.trim();
        (!name.is_empty()).then(|| name.to_string())
    }

    pub fn signed_in(&self, account: Option<&str>) -> Option<String> {
        match account {
            Some(account) => {
                read_json::<StoredSession>(&self.session_path(account)).map(|s| s.account)
            }
            None => self.last_account(),
        }
    }

    pub fn last(&self) -> Option<StoredSession> {
        self.load(&self.last_account()?)
    }

    pub fn save(&self, session: &StoredSession) -> Result<()> {
        self.write_private(
            &self.session_path(&session.account),
            &serde_json::to_vec(session)?,
        )?;
        self.write_private(&self.dir.join(LAST), session.account.as_bytes())
    }

    pub fn sign_out<T>(
        &self,
        account: &str,
        revoke: impl FnOnce(Option<&StoredSession>) -> T,
    ) -> Result<T> {
        let lock = self.lock(account, LockMode::Exclusive)?;
        let revoked = revoke(read_json(&self.session_path(account)).as_ref());
        self.delete(account)?;
        drop(lock);
        Ok(revoked)
    }

    pub fn take_warnings(&self) -> impl Iterator<Item = StoreWarning> {
        let pending = self.pending.swap(0, Ordering::Relaxed);
        self.reported.fetch_or(pending, Ordering::Relaxed);
        StoreWarning::ALL
            .into_iter()
            .filter(move |warning| pending & warning.bit() != 0)
    }

    fn warn(&self, warning: StoreWarning) {
        if self.reported.load(Ordering::Relaxed) & warning.bit() == 0 {
            self.pending.fetch_or(warning.bit(), Ordering::Relaxed);
        }
    }

    pub fn delete(&self, account: &str) -> Result<()> {
        remove_if_present(&self.session_path(account))?;
        if self.last_account().as_deref() == Some(account) {
            remove_if_present(&self.dir.join(LAST))?;
        }
        Ok(())
    }

    pub fn cached_client(&self, registration_endpoint: &str) -> Option<String> {
        read_json::<CachedClient>(&self.dir.join(CLIENT))
            .filter(|cached| cached.registration_endpoint == registration_endpoint)
            .map(|cached| cached.client_id)
    }

    pub fn save_client(&self, client: &CachedClient) -> Result<()> {
        self.write_private(&self.dir.join(CLIENT), &serde_json::to_vec(client)?)
    }

    pub fn forget_client(&self) -> Result<()> {
        remove_if_present(&self.dir.join(CLIENT))
    }

    pub fn refresh(
        &self,
        held: &StoredSession,
        refresh: impl FnOnce(&StoredSession) -> Result<StoredSession>,
    ) -> Result<StoredSession> {
        let path = self.session_path(&held.account);
        let lock = self.lock(&held.account, LockMode::Exclusive)?;
        let latest = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<StoredSession>(&bytes)
                .ok()
                .filter(|s| s.client_id == held.client_id && s.obtained_at >= held.obtained_at),
            Err(e) if e.kind() == ErrorKind::NotFound => return Err(Error::SessionExpired),
            Err(e) => return Err(e.into()),
        };
        if let Some(latest) = &latest
            && latest != held
            && latest.refresh_at().is_none_or(|at| SystemTime::now() < at)
        {
            return Ok(latest.clone());
        }
        let result = refresh(latest.as_ref().unwrap_or(held));
        match &result {
            Ok(fresh) => {
                if self
                    .write_private(&path, &serde_json::to_vec(fresh)?)
                    .is_err()
                {
                    self.warn(StoreWarning::NotSaved);
                }
            }
            Err(Error::SessionExpired) => remove_if_present(&path)?,
            Err(_) => {}
        }
        drop(lock);
        result
    }

    fn lock(&self, account: &str, mode: LockMode) -> Result<Option<File>> {
        let mut path = self.session_path(account).into_os_string();
        path.push(LOCK_SUFFIX);
        let file = private_options().write(true).truncate(false).open(path)?;
        let locked = match mode {
            LockMode::Shared => file.lock_shared(),
            LockMode::Exclusive => file.lock(),
        };
        match locked {
            Ok(()) => Ok(Some(file)),
            Err(e) if e.kind() == ErrorKind::Unsupported => {
                self.warn(StoreWarning::Unlocked);
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn session_path(&self, account: &str) -> PathBuf {
        let mut name = hash_b64(account.trim().to_lowercase().as_bytes());
        name.push_str(SESSION_SUFFIX);
        self.dir.join(name)
    }

    fn write_private(&self, target: &Path, bytes: &[u8]) -> Result<()> {
        let tmp = tmp_path(target, self.writer_id, &self.counter);
        let written = private_options()
            .write(true)
            .truncate(true)
            .open(&tmp)
            .and_then(|mut file| file.write_all(bytes))
            .and_then(|()| fs::rename(&tmp, target));
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
}

#[cfg(unix)]
fn restrict_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(Error::from)
}

#[cfg(not(unix))]
fn restrict_dir(_dir: &Path) -> Result<()> {
    Ok(())
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/auth/store/tests.rs"]
mod tests;
