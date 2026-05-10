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
    AUTHORIZATION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue, LOCATION,
};
use reqwest::redirect::Policy;
use reqwest::{Method, StatusCode};
use std::fmt::Write as _;
use std::io::Read;
use std::time::Duration;

const SNIPPET_MAX_BYTES: usize = 200;

pub struct HttpClient {
    base_url: String,
    inner: Client,
    auth_header: HeaderValue,
    debug: bool,
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
            debug: config.debug,
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
        let url = self.url(path);
        if self.debug {
            eprintln!("[debug] -> GET {url}");
        }
        let resp = self
            .inner
            .request(Method::GET, &url)
            .header(AUTHORIZATION, self.auth_header.clone())
            .send()?;
        self.log_response("GET", &url, &resp);
        Ok(resp)
    }

    pub fn post_json(&self, path: &str, body: &serde_json::Value) -> CliResult<Response> {
        let url = self.url(path);
        if self.debug {
            eprintln!("[debug] -> POST {url}");
        }
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, self.auth_header.clone());
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let resp = self
            .inner
            .request(Method::POST, &url)
            .headers(headers)
            .json(body)
            .send()?;
        self.log_response("POST", &url, &resp);
        Ok(resp)
    }

    fn log_response(&self, method: &str, url: &str, resp: &Response) {
        if !self.debug {
            return;
        }
        let status = resp.status();
        let ct = header_str(resp.headers().get(CONTENT_TYPE)).unwrap_or("(none)");
        let cl = header_str(resp.headers().get(CONTENT_LENGTH)).unwrap_or("?");
        let ce = header_str(resp.headers().get(CONTENT_ENCODING)).unwrap_or("identity");
        eprintln!(
            "[debug] <- {method} {url} status={status} content-type={ct} content-length={cl} content-encoding={ce}"
        );
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
            if let Some((hash, bytes)) = cache.any_cached() {
                let ansi = Ansi::new(color);
                eprintln!(
                    "{}warning:{} failed to refresh schema ({e}); using cached copy",
                    ansi.yellow(),
                    ansi.reset(),
                );
                match serde_json::from_slice::<Schema>(&bytes) {
                    Ok(schema) => Ok(schema),
                    Err(parse_err) => {
                        let file = cache.file_for_hash(&hash).display().to_string();
                        cache.invalidate(&hash);
                        Err(CliError::SchemaCacheCorrupt {
                            file,
                            reason: parse_err.to_string(),
                            byte_len: bytes.len(),
                            snippet: snippet(&bytes, SNIPPET_MAX_BYTES),
                        })
                    }
                }
            } else {
                Err(e)
            }
        }
    }
}

fn fetch_schema_inner(client: &HttpClient, cache: &SchemaCache) -> CliResult<Schema> {
    let path = "/api/schema";
    let resp = client.get_raw(path)?;
    let status = resp.status();

    if status.is_redirection() {
        let hash = extract_hash_from_location(&resp)?;
        if cache.latest_hash().as_deref() == Some(hash.as_str())
            && let Some(bytes) = cache.read(&hash)
            && let Ok(schema) = serde_json::from_slice::<Schema>(&bytes)
        {
            return Ok(schema);
        }
        let hashed_path = format!("/api/schema/{}", hash);
        let body = client.get_raw(&hashed_path)?;
        let body_status = body.status();
        if !body_status.is_success() {
            return Err(status_error(body_status));
        }
        let content_type = response_content_type(&body);
        let decoded = read_schema_body(body)?;
        let schema = parse_schema(&decoded, &hashed_path, body_status.as_u16(), &content_type)?;
        cache.write(&hash, &decoded)?;
        Ok(schema)
    } else if status.is_success() {
        let content_type = response_content_type(&resp);
        let decoded = read_schema_body(resp)?;
        let hash = hash_b64(&decoded);
        let schema = parse_schema(&decoded, path, status.as_u16(), &content_type)?;
        cache.write(&hash, &decoded)?;
        Ok(schema)
    } else {
        Err(status_error(status))
    }
}

fn parse_schema(bytes: &[u8], path: &str, status: u16, content_type: &str) -> CliResult<Schema> {
    serde_json::from_slice(bytes).map_err(|e| CliError::SchemaParse {
        path: path.to_string(),
        reason: e.to_string(),
        status,
        content_type: content_type.to_string(),
        byte_len: bytes.len(),
        snippet: snippet(bytes, SNIPPET_MAX_BYTES),
    })
}

fn response_content_type(resp: &Response) -> String {
    header_str(resp.headers().get(CONTENT_TYPE))
        .unwrap_or("")
        .to_string()
}

fn header_str(value: Option<&HeaderValue>) -> Option<&str> {
    value.and_then(|v| v.to_str().ok())
}

fn read_schema_body(resp: Response) -> CliResult<Vec<u8>> {
    let encoding = header_str(resp.headers().get(CONTENT_ENCODING))
        .map(|s| s.trim().to_ascii_lowercase());
    let bytes = resp.bytes()?;
    decode_schema_body(&bytes, encoding.as_deref())
}

fn decode_schema_body(bytes: &[u8], content_encoding: Option<&str>) -> CliResult<Vec<u8>> {
    match content_encoding {
        Some("gzip") | Some("x-gzip") => gunzip(bytes),
        Some("identity") | Some("") | None => {
            if has_gzip_magic(bytes) {
                gunzip(bytes)
            } else {
                Ok(bytes.to_vec())
            }
        }
        Some(other) => Err(CliError::UnexpectedResponse(format!(
            "unsupported Content-Encoding: {other}"
        ))),
    }
}

fn has_gzip_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b
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

fn snippet(bytes: &[u8], max_bytes: usize) -> String {
    let truncated = if bytes.len() > max_bytes {
        &bytes[..max_bytes]
    } else {
        bytes
    };
    let lossy = String::from_utf8_lossy(truncated);
    let mut out = String::with_capacity(lossy.len() + 4);
    out.push('"');
    for c in lossy.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\u{{{:04x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    if bytes.len() > max_bytes {
        out.push_str(" (truncated)");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn decodes_gzip_with_content_encoding_header() {
        let plain = br#"{"schema":true}"#;
        let compressed = gzip(plain);
        let decoded = decode_schema_body(&compressed, Some("gzip")).unwrap();
        assert_eq!(decoded, plain);
    }

    #[test]
    fn accepts_plain_json_without_content_encoding() {
        let plain = br#"{"schema":true}"#;
        let decoded = decode_schema_body(plain, None).unwrap();
        assert_eq!(decoded, plain);
    }

    #[test]
    fn falls_back_to_gzip_magic_without_content_encoding() {
        let plain = br#"{"schema":true}"#;
        let compressed = gzip(plain);
        let decoded = decode_schema_body(&compressed, None).unwrap();
        assert_eq!(decoded, plain);
    }

    #[test]
    fn accepts_plain_json_with_identity_encoding() {
        let plain = br#"{"schema":true}"#;
        let decoded = decode_schema_body(plain, Some("identity")).unwrap();
        assert_eq!(decoded, plain);
    }

    #[test]
    fn rejects_unsupported_content_encoding() {
        let err = decode_schema_body(b"anything", Some("br")).unwrap_err();
        assert!(matches!(err, CliError::UnexpectedResponse(_)));
    }

    #[test]
    fn snippet_quotes_short_text() {
        assert_eq!(snippet(b"hello", 200), "\"hello\"");
    }

    #[test]
    fn snippet_escapes_control_characters() {
        assert_eq!(
            snippet(b"a\nb\tc\rd", 200),
            "\"a\\nb\\tc\\rd\"",
        );
    }

    #[test]
    fn snippet_escapes_quotes_and_backslashes() {
        assert_eq!(snippet(br#"a"\b"#, 200), "\"a\\\"\\\\b\"");
    }

    #[test]
    fn snippet_truncates_long_input() {
        let long = b"x".repeat(500);
        let s = snippet(&long, 200);
        assert!(s.starts_with('"'));
        assert!(s.ends_with("(truncated)"));
        assert_eq!(s.matches('x').count(), 200);
    }

    #[test]
    fn snippet_handles_invalid_utf8() {
        let bytes: &[u8] = &[b'a', 0xff, 0xfe, b'b'];
        let s = snippet(bytes, 200);
        assert!(s.starts_with("\"a"));
        assert!(s.ends_with("b\""));
    }

    #[test]
    fn schema_parse_error_includes_diagnostics() {
        let err = parse_schema(b"<!doctype html>", "/api/schema", 200, "text/html");
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("/api/schema"), "{msg}");
        assert!(msg.contains("status: 200"), "{msg}");
        assert!(msg.contains("text/html"), "{msg}");
        assert!(msg.contains("body length: 15 bytes"), "{msg}");
        assert!(msg.contains("<!doctype html>"), "{msg}");
    }

    #[test]
    fn schema_parse_error_on_empty_body() {
        let err = parse_schema(b"", "/api/schema", 200, "");
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("body length: 0 bytes"), "{msg}");
        assert!(msg.contains("body snippet: \"\""), "{msg}");
    }
}
