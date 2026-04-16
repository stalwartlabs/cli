/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::error::{CliError, CliResult};
use crate::jmap::http::{HttpClient, status_error};
use serde::Deserialize;

#[derive(Debug)]
pub struct Session {
    pub api_path: String,
    pub max_objects_in_get: usize,
    pub max_objects_in_set: usize,
}

#[derive(Deserialize)]
struct RawSession {
    #[serde(default)]
    capabilities: RawCapabilities,
    #[serde(rename = "apiUrl")]
    api_url: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawCapabilities {
    #[serde(rename = "urn:ietf:params:jmap:core")]
    core: Option<RawCore>,
}

#[derive(Deserialize)]
struct RawCore {
    #[serde(rename = "maxObjectsInGet")]
    max_objects_in_get: Option<usize>,
    #[serde(rename = "maxObjectsInSet")]
    max_objects_in_set: Option<usize>,
}

impl Session {
    pub fn fetch(client: &HttpClient) -> CliResult<Self> {
        let resp = client.get_raw("/jmap/session")?;
        let status = resp.status();
        if !status.is_success() {
            return Err(status_error(status));
        }
        let raw: RawSession = resp.json()?;
        let core = raw.capabilities.core.ok_or_else(|| {
            CliError::UnexpectedResponse("session missing urn:ietf:params:jmap:core".into())
        })?;
        let api_url = raw
            .api_url
            .ok_or_else(|| CliError::UnexpectedResponse("session missing apiUrl".into()))?;
        let api_path = absolute_to_path(&api_url);

        Ok(Session {
            api_path,
            max_objects_in_get: core.max_objects_in_get.unwrap_or(500),
            max_objects_in_set: core.max_objects_in_set.unwrap_or(500),
        })
    }
}

fn absolute_to_path(url: &str) -> String {
    let after = match url.split_once("://") {
        Some((scheme, rest))
            if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") =>
        {
            rest
        }
        _ => return url.trim_end_matches('/').to_string(),
    };
    let path_start = after.find('/').unwrap_or(after.len());
    let path = &after[path_start..];
    if path.is_empty() {
        "/".to_string()
    } else {
        path.trim_end_matches('/').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_url_becomes_path() {
        assert_eq!(absolute_to_path("http://127.0.0.1:8080/jmap/"), "/jmap");
        assert_eq!(absolute_to_path("https://mail.example.com/jmap"), "/jmap");
    }

    #[test]
    fn relative_url_trimmed() {
        assert_eq!(absolute_to_path("/jmap/"), "/jmap");
        assert_eq!(absolute_to_path("/jmap"), "/jmap");
    }
}
