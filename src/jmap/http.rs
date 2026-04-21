/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::config::Config;
use crate::app::error::{CliError, CliResult};
use crate::jmap::auth;
use crate::jmap::cache::{SchemaCache, hash_b64};
use crate::render::Ansi;
use crate::schema::Schema;
use flate2::read::GzDecoder;
use reqwest::blocking::{Client, Response};
use reqwest::header::{
    AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE, HeaderMap, HeaderValue, LOCATION,
};
use reqwest::redirect::Policy;
use reqwest::{Method, StatusCode};
use std::io::Read;
use std::time::Duration;

pub struct HttpClient {
    base_url: String,
    inner: Client,
    auth_header: HeaderValue,
}

impl HttpClient {
    pub fn new(config: &Config) -> CliResult<Self> {
        let auth_header = HeaderValue::from_str(&auth::header_value(&config.auth))
            .map_err(|e| CliError::msg(format!("invalid auth header: {e}")))?;

        let mut builder = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .no_gzip();

        if config.insecure {
            builder = builder
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true);
        }

        let inner = builder.build()?;

        Ok(HttpClient {
            base_url: config.url.clone(),
            inner,
            auth_header,
        })
    }

    fn url(&self, path: &str) -> String {
        let mut out = String::with_capacity(self.base_url.len() + path.len() + 1);
        out.push_str(&self.base_url);
        if !path.starts_with('/') {
            out.push('/');
        }
        out.push_str(path);
        out
    }

    pub fn get_raw(&self, path: &str) -> CliResult<Response> {
        let resp = self
            .inner
            .request(Method::GET, self.url(path))
            .header(AUTHORIZATION, self.auth_header.clone())
            .send()?;
        Ok(resp)
    }

    pub fn post_json(&self, path: &str, body: &serde_json::Value) -> CliResult<Response> {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, self.auth_header.clone());
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let resp = self
            .inner
            .request(Method::POST, self.url(path))
            .headers(headers)
            .json(body)
            .send()?;
        Ok(resp)
    }
}

pub fn status_error(status: StatusCode) -> CliError {
    match status.as_u16() {
        401 => CliError::AuthFailed,
        403 => CliError::PermissionDenied,
        404 => CliError::NotFound,
        429 => CliError::RateLimited,
        code => CliError::HttpStatus(code),
    }
}

pub fn fetch_schema(client: &HttpClient, cache: &SchemaCache, color: bool) -> CliResult<Schema> {
    match fetch_schema_inner(client, cache) {
        Ok(s) => Ok(s),
        Err(e) => {
            if let Some((_hash, bytes)) = cache.any_cached() {
                let ansi = Ansi::new(color);
                eprintln!(
                    "{}warning:{} failed to refresh schema ({e}); using cached copy",
                    ansi.yellow(),
                    ansi.reset(),
                );
                let schema: Schema = serde_json::from_slice(&bytes)?;
                return Ok(schema);
            }
            Err(e)
        }
    }
}

fn fetch_schema_inner(client: &HttpClient, cache: &SchemaCache) -> CliResult<Schema> {
    let resp = client.get_raw("/api/schema")?;
    let status = resp.status();

    if status.is_redirection() {
        let hash = extract_hash_from_location(&resp)?;
        if cache.latest_hash().as_deref() == Some(hash.as_str())
            && let Some(bytes) = cache.read(&hash)
        {
            let schema: Schema = serde_json::from_slice(&bytes)?;
            return Ok(schema);
        }
        let body = client.get_raw(&format!("/api/schema/{}", hash))?;
        if !body.status().is_success() {
            return Err(status_error(body.status()));
        }
        let content_encoding = body
            .headers()
            .get(CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = body.bytes()?;
        let decoded = decode_schema(&bytes, content_encoding.as_deref())?;
        cache.write(&hash, &decoded)?;
        let schema: Schema = serde_json::from_slice(&decoded)?;
        Ok(schema)
    } else if status.is_success() {
        let content_encoding = resp
            .headers()
            .get(CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = resp.bytes()?;
        let decoded = decode_schema(&bytes, content_encoding.as_deref())?;
        let hash = hash_b64(&decoded);
        cache.write(&hash, &decoded)?;
        let schema: Schema = serde_json::from_slice(&decoded)?;
        Ok(schema)
    } else {
        Err(status_error(status))
    }
}

fn extract_hash_from_location(resp: &Response) -> CliResult<String> {
    let loc = resp
        .headers()
        .get(LOCATION)
        .ok_or_else(|| CliError::UnexpectedResponse("redirect without Location header".into()))?
        .to_str()
        .map_err(|e| CliError::UnexpectedResponse(format!("invalid Location header: {e}")))?;
    let hash = loc
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CliError::UnexpectedResponse(format!("unexpected Location format: {loc}"))
        })?;
    Ok(hash.to_string())
}

fn gunzip(bytes: &[u8]) -> CliResult<Vec<u8>> {
    let mut dec = GzDecoder::new(bytes);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)?;
    Ok(out)
}

fn decode_schema(bytes: &[u8], content_encoding: Option<&str>) -> CliResult<Vec<u8>> {
    if is_gzip_encoded(bytes, content_encoding) {
        gunzip(bytes)
    } else {
        Ok(bytes.to_vec())
    }
}

fn is_gzip_encoded(bytes: &[u8], content_encoding: Option<&str>) -> bool {
    content_encoding
        .map(|value| {
            value
                .split(',')
                .any(|encoding| encoding.trim().eq_ignore_ascii_case("gzip"))
        })
        .unwrap_or_else(|| bytes.starts_with(&[0x1f, 0x8b]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    #[test]
    fn decodes_gzipped_schema() {
        let schema = br#"{"objects":{}}"#;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(schema).unwrap();
        let encoded = encoder.finish().unwrap();

        assert_eq!(decode_schema(&encoded, Some("gzip")).unwrap(), schema);
    }

    #[test]
    fn accepts_plain_schema() {
        let schema = br#"{"objects":{}}"#;

        assert_eq!(decode_schema(schema, None).unwrap(), schema);
    }

    #[test]
    fn decodes_gzipped_schema_without_header() {
        let schema = br#"{"objects":{}}"#;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(schema).unwrap();
        let encoded = encoder.finish().unwrap();

        assert_eq!(decode_schema(&encoded, None).unwrap(), schema);
    }
}
