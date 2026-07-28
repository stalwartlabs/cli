/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::error::{CliError, CliResult};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub fn hash_b64(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    URL_SAFE_NO_PAD.encode(digest)
}

pub fn normalise_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some((scheme_raw, rest)) = trimmed.split_once("://") else {
        return trimmed.trim_end_matches('/').to_string();
    };

    let scheme_lc = scheme_raw.to_ascii_lowercase();
    let host_and_port = match rest.find(['/', '?', '#']) {
        Some(i) => &rest[..i],
        None => rest,
    };
    let host_and_port_lc = host_and_port.to_ascii_lowercase();

    let cleaned = if scheme_lc == "http" || scheme_lc == "https" {
        strip_default_port(&host_and_port_lc, &scheme_lc)
    } else {
        host_and_port_lc
    };

    let mut out = String::with_capacity(scheme_lc.len() + 3 + cleaned.len());
    out.push_str(&scheme_lc);
    out.push_str("://");
    out.push_str(&cleaned);
    out
}

fn strip_default_port(host_and_port: &str, scheme: &str) -> String {
    if let Some((host, port)) = host_and_port.rsplit_once(':') {
        if host.starts_with('[') && !host.ends_with(']') {
            return host_and_port.to_string();
        }
        let default = match scheme {
            "https" => "443",
            "http" => "80",
            _ => "",
        };
        if port == default {
            return host.to_string();
        }
    }
    host_and_port.to_string()
}

pub struct SchemaCache {
    dir: PathBuf,
}

impl SchemaCache {
    pub fn new(server_url: &str) -> CliResult<Self> {
        let base = dirs::cache_dir()
            .ok_or_else(|| CliError::msg("could not determine user cache directory"))?;
        let server_hash = hash_b64(normalise_url(server_url).as_bytes());
        let dir = base.join("stalwart-cli").join(server_hash);
        fs::create_dir_all(&dir)?;
        Ok(SchemaCache { dir })
    }

    pub fn latest_hash(&self) -> Option<String> {
        fs::read_to_string(self.dir.join("latest"))
            .ok()
            .map(|s| s.trim().to_string())
    }

    pub fn read(&self, hash: &str) -> Option<Vec<u8>> {
        fs::read(self.file_for(hash)).ok()
    }

    pub fn any_cached(&self) -> Option<(String, Vec<u8>)> {
        let hash = self.latest_hash()?;
        let bytes = self.read(&hash)?;
        Some((hash, bytes))
    }

    pub fn write(&self, hash: &str, bytes: &[u8]) -> CliResult<()> {
        let target = self.file_for(hash);
        let tmp = with_suffix(&target, &tmp_suffix());
        write_then_rename(&tmp, &target, bytes)?;

        let latest = self.dir.join("latest");
        let latest_tmp = self.dir.join(format!("latest{}", tmp_suffix()));
        write_then_rename(&latest_tmp, &latest, hash.as_bytes())?;
        Ok(())
    }

    pub fn file_for_hash(&self, hash: &str) -> PathBuf {
        self.file_for(hash)
    }

    pub fn invalidate(&self, hash: &str) {
        let _ = fs::remove_file(self.dir.join("latest"));
        let _ = fs::remove_file(self.file_for(hash));
    }

    fn file_for(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("schema-{}.json", hash))
    }
}

fn write_then_rename(tmp: &Path, target: &Path, bytes: &[u8]) -> CliResult<()> {
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

fn tmp_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        ".tmp.{}.{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names_are_unique_per_writer() {
        let a = tmp_suffix();
        let b = tmp_suffix();
        assert_ne!(
            a, b,
            "two writers sharing a temp path can rename a half-written file into place"
        );
        assert!(a.contains(&std::process::id().to_string()));
    }

    #[test]
    fn concurrent_writes_leave_a_complete_file() {
        let dir = std::env::temp_dir().join(format!("stalwart-cli-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cache = SchemaCache { dir: dir.clone() };

        let bytes = vec![b'x'; 512 * 1024];
        std::thread::scope(|s| {
            for _ in 0..8 {
                let cache = &cache;
                let bytes = &bytes;
                s.spawn(move || cache.write("h", bytes).unwrap());
            }
        });

        assert_eq!(
            cache.read("h").as_deref(),
            Some(bytes.as_slice()),
            "a reader must never observe a partially written schema"
        );
        assert_eq!(cache.latest_hash().as_deref(), Some("h"));
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files must not be left behind");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalise_lowercases_scheme_and_host() {
        assert_eq!(
            normalise_url("HTTPS://Mail.Example.com/"),
            "https://mail.example.com"
        );
    }

    #[test]
    fn normalise_strips_default_https_port() {
        assert_eq!(
            normalise_url("https://mail.example.com:443"),
            "https://mail.example.com"
        );
    }

    #[test]
    fn normalise_keeps_non_default_port() {
        assert_eq!(
            normalise_url("http://localhost:8080/"),
            "http://localhost:8080"
        );
    }

    #[test]
    fn normalise_drops_path_and_query() {
        assert_eq!(
            normalise_url("https://mail.example.com/foo/bar?x=1#y"),
            "https://mail.example.com"
        );
    }

    #[test]
    fn hash_b64_produces_43_chars() {
        let h = hash_b64(b"hello");
        assert_eq!(h.len(), 43);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }
}
