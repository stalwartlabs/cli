/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::jmap::cache::{SchemaCache, hash_b64};
use crate::jmap::http::{HttpClient, header_str};
use crate::schema::Schema;
use flate2::read::GzDecoder;
use reqwest::blocking::Response;
use reqwest::header::{CONTENT_ENCODING, CONTENT_TYPE, LOCATION};
use std::fmt::Write as _;
use std::io::Read;

const SCHEMA_PATH: &str = "/api/schema";
const SNIPPET_MAX_BYTES: usize = 200;
const GZIP_MAGIC: &[u8] = &[0x1f, 0x8b];

pub struct FetchedSchema {
    pub schema: Schema,
    pub stale: Option<Error>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContentEncoding {
    Gzip,
    Identity,
}

pub fn fetch(http: &HttpClient, cache: &SchemaCache) -> Result<FetchedSchema> {
    let refresh = match fetch_fresh(http, cache) {
        Ok(schema) => {
            return Ok(FetchedSchema {
                schema,
                stale: None,
            });
        }
        Err(e) => e,
    };
    let Some((hash, bytes)) = cache.any_cached() else {
        return Err(refresh);
    };
    match serde_json::from_slice::<Schema>(&bytes) {
        Ok(schema) => Ok(FetchedSchema {
            schema,
            stale: Some(refresh),
        }),
        Err(parse_err) => {
            let file = cache.file_for_hash(&hash).display().to_string();
            cache.invalidate(&hash);
            Err(Error::SchemaCacheCorrupt {
                file,
                reason: parse_err.to_string(),
                byte_len: bytes.len(),
                snippet: snippet(&bytes, SNIPPET_MAX_BYTES),
                refresh: Box::new(refresh),
            })
        }
    }
}

fn fetch_fresh(http: &HttpClient, cache: &SchemaCache) -> Result<Schema> {
    let resp = http.get_any_status(SCHEMA_PATH)?;
    let status = resp.status();

    if status.is_redirection() {
        let hash = hash_from_location(&resp)?;
        if cache.latest_hash().as_deref() == Some(hash)
            && let Some(bytes) = cache.read(hash)
            && let Ok(schema) = serde_json::from_slice::<Schema>(&bytes)
        {
            return Ok(schema);
        }
        let hashed_path = format!("{SCHEMA_PATH}/{hash}");
        let body = http.get(&hashed_path)?;
        let body_status = body.status().as_u16();
        let content_type = response_content_type(&body);
        let decoded = read_schema_body(body)?;
        let schema = parse_schema(&decoded, &hashed_path, body_status, &content_type)?;
        cache.write(hash, &decoded)?;
        Ok(schema)
    } else if status.is_success() {
        let content_type = response_content_type(&resp);
        let decoded = read_schema_body(resp)?;
        let hash = hash_b64(&decoded);
        let schema = parse_schema(&decoded, SCHEMA_PATH, status.as_u16(), &content_type)?;
        cache.write(&hash, &decoded)?;
        Ok(schema)
    } else {
        Err(status.into())
    }
}

fn parse_schema(bytes: &[u8], path: &str, status: u16, content_type: &str) -> Result<Schema> {
    serde_json::from_slice(bytes).map_err(|e| Error::SchemaParse {
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

fn read_schema_body(resp: Response) -> Result<Vec<u8>> {
    let encoding = ContentEncoding::from_header(header_str(resp.headers().get(CONTENT_ENCODING)));
    let bytes = resp.bytes()?;
    encoding?.decode(&bytes)
}

impl ContentEncoding {
    fn from_header(value: Option<&str>) -> Result<Self> {
        let Some(raw) = value.map(str::trim).filter(|raw| !raw.is_empty()) else {
            return Ok(ContentEncoding::Identity);
        };
        hashify::map_ignore_case!(
            raw.as_bytes(),
            ContentEncoding,
            "gzip" => ContentEncoding::Gzip,
            "x-gzip" => ContentEncoding::Gzip,
            "identity" => ContentEncoding::Identity,
        )
        .copied()
        .ok_or_else(|| unsupported_encoding(raw))
    }

    fn decode(self, bytes: &[u8]) -> Result<Vec<u8>> {
        match self {
            ContentEncoding::Gzip => gunzip(bytes),
            ContentEncoding::Identity if bytes.starts_with(GZIP_MAGIC) => gunzip(bytes),
            ContentEncoding::Identity => Ok(bytes.to_vec()),
        }
    }
}

#[cold]
fn unsupported_encoding(raw: &str) -> Error {
    Error::UnexpectedResponse(format!(
        "unsupported Content-Encoding: {}",
        raw.to_ascii_lowercase()
    ))
}

fn hash_from_location(resp: &Response) -> Result<&str> {
    let loc = resp
        .headers()
        .get(LOCATION)
        .ok_or_else(|| Error::UnexpectedResponse("redirect without Location header".into()))?
        .to_str()
        .map_err(|e| Error::UnexpectedResponse(format!("invalid Location header: {e}")))?;
    let hash = loc.rsplit_once('/').map_or(loc, |(_, tail)| tail);
    if hash.is_empty() {
        return Err(Error::UnexpectedResponse(format!(
            "unexpected Location format: {loc}"
        )));
    }
    Ok(hash)
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    GzDecoder::new(bytes).read_to_end(&mut out)?;
    Ok(out)
}

fn snippet(bytes: &[u8], max_bytes: usize) -> String {
    let truncated = bytes.get(..max_bytes).unwrap_or(bytes);
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
            c if c < '\u{20}' || c == '\u{7f}' => {
                let _ = write!(out, "\\u{{{:04x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    if truncated.len() < bytes.len() {
        out.push_str(" (truncated)");
    }
    out
}
#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/schema_fetch/tests.rs"]
mod tests;
