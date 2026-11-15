/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use encodify::base64::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const CACHE_DIR_NAME: &str = "stalwart-cli";
const LATEST: &str = "latest";
const SCHEMA_FILE_PREFIX: &str = "schema-";
const SCHEMA_FILE_SUFFIX: &str = ".json";

pub fn hash_b64(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    URL_SAFE_NO_PAD.encode(digest)
}

pub fn normalise_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.trim_end_matches('/').to_string();
    };

    let host_and_port = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = match default_port(scheme) {
        Some(port) => strip_port(host_and_port, port),
        None => host_and_port,
    };

    let mut out = String::with_capacity(scheme.len() + 3 + host.len());
    out.push_str(scheme);
    out.push_str("://");
    out.push_str(host);
    out.make_ascii_lowercase();
    out
}

fn default_port(scheme: &str) -> Option<&'static str> {
    hashify::map_ignore_case!(scheme.as_bytes(), &'static str, "https" => "443", "http" => "80")
        .copied()
}

fn strip_port<'a>(host_and_port: &'a str, default: &str) -> &'a str {
    match host_and_port.rsplit_once(':') {
        Some((host, _)) if host.starts_with('[') && !host.ends_with(']') => host_and_port,
        Some((host, port)) if port == default => host,
        _ => host_and_port,
    }
}

pub struct SchemaCache {
    dir: PathBuf,
    writer_id: u32,
    tmp_counter: AtomicU64,
}

impl SchemaCache {
    pub fn new(base: &Path, server_url: &str, writer_id: u32) -> Result<Self> {
        let server_hash = hash_b64(normalise_url(server_url).as_bytes());
        let dir = base.join(CACHE_DIR_NAME).join(server_hash);
        fs::create_dir_all(&dir)?;
        Ok(SchemaCache::at(dir, writer_id))
    }

    fn at(dir: PathBuf, writer_id: u32) -> Self {
        SchemaCache {
            dir,
            writer_id,
            tmp_counter: AtomicU64::new(0),
        }
    }

    pub fn latest_hash(&self) -> Option<String> {
        fs::read_to_string(self.dir.join(LATEST))
            .ok()
            .map(|s| s.trim().to_string())
    }

    pub fn read(&self, hash: &str) -> Option<Vec<u8>> {
        fs::read(self.file_for_hash(hash)).ok()
    }

    pub fn any_cached(&self) -> Option<(String, Vec<u8>)> {
        let hash = self.latest_hash()?;
        let bytes = self.read(&hash)?;
        Some((hash, bytes))
    }

    pub fn write(&self, hash: &str, bytes: &[u8]) -> Result<()> {
        let target = self.file_for_hash(hash);
        write_then_rename(&self.tmp_path(&target), &target, bytes)?;

        let latest = self.dir.join(LATEST);
        write_then_rename(&self.tmp_path(&latest), &latest, hash.as_bytes())
    }

    pub fn file_for_hash(&self, hash: &str) -> PathBuf {
        let mut name =
            String::with_capacity(SCHEMA_FILE_PREFIX.len() + hash.len() + SCHEMA_FILE_SUFFIX.len());
        name.push_str(SCHEMA_FILE_PREFIX);
        name.push_str(hash);
        name.push_str(SCHEMA_FILE_SUFFIX);
        self.dir.join(name)
    }

    pub fn invalidate(&self, hash: &str) {
        let _ = fs::remove_file(self.dir.join(LATEST));
        let _ = fs::remove_file(self.file_for_hash(hash));
    }

    fn tmp_path(&self, target: &Path) -> PathBuf {
        tmp_path(target, self.writer_id, &self.tmp_counter)
    }
}

pub fn tmp_path(target: &Path, writer_id: u32, counter: &AtomicU64) -> PathBuf {
    let mut suffix = String::with_capacity(32);
    let _ = write!(
        suffix,
        ".tmp.{}.{}",
        writer_id,
        counter.fetch_add(1, Ordering::Relaxed)
    );
    let mut path = target.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

fn write_then_rename(tmp: &Path, target: &Path, bytes: &[u8]) -> Result<()> {
    if let Err(e) = fs::write(tmp, bytes) {
        let _ = fs::remove_file(tmp);
        return Err(e.into());
    }
    if let Err(e) = fs::rename(tmp, target) {
        let _ = fs::remove_file(tmp);
        return Err(e.into());
    }
    Ok(())
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/cache/tests.rs"]
mod tests;
