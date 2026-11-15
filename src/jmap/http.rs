/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::jmap::credential::Credential;
use bytes::Bytes;
use log::{Level, debug, log_enabled};
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue,
};
use reqwest::redirect::Policy;
use reqwest::{Method, StatusCode};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub const HTTP_TRACE: &str = "stalwart_cli::http";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const JSON: &str = "application/json";
const FORM: &str = "application/x-www-form-urlencoded";
const EVENT_STREAM: &str = "text/event-stream";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsVerify {
    Verify,
    Insecure,
}

pub struct ConnectOptions {
    pub url: String,
    pub credential: Arc<Credential>,
    pub tls: TlsVerify,
}

pub struct HttpClient {
    base_url: String,
    inner: Client,
    credential: Option<Arc<Credential>>,
    bounded_timeout: Duration,
}

struct Body {
    bytes: Bytes,
    content_type: &'static str,
}

impl HttpClient {
    pub fn new(opts: &ConnectOptions) -> Result<Self> {
        Self::build(
            &opts.url,
            Some(opts.credential.clone()),
            opts.tls,
            Some(REQUEST_TIMEOUT),
        )
    }

    pub fn streaming(opts: &ConnectOptions) -> Result<Self> {
        Self::build(&opts.url, Some(opts.credential.clone()), opts.tls, None)
    }

    pub fn anonymous(url: &str, tls: TlsVerify) -> Result<Self> {
        Self::build(url, None, tls, Some(REQUEST_TIMEOUT))
    }

    fn build(
        url: &str,
        credential: Option<Arc<Credential>>,
        tls: TlsVerify,
        timeout: Option<Duration>,
    ) -> Result<Self> {
        let mut builder = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .no_gzip();

        if tls == TlsVerify::Insecure {
            builder = builder
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true);
        }

        Ok(HttpClient {
            base_url: url.trim_end_matches('/').to_string(),
            inner: builder.build()?,
            credential,
            bounded_timeout: REQUEST_TIMEOUT,
        })
    }

    pub fn resolve(&self, endpoint: &str) -> String {
        if is_absolute(endpoint) {
            return endpoint.to_string();
        }
        let mut out = String::with_capacity(self.base_url.len() + endpoint.len() + 1);
        out.push_str(&self.base_url);
        if !endpoint.starts_with('/') {
            out.push('/');
        }
        out.push_str(endpoint);
        out
    }

    pub fn get_any_status(&self, path: &str) -> Result<Response> {
        self.send(Method::GET, &self.resolve(path), None, None)
    }

    pub fn get(&self, path: &str) -> Result<Response> {
        success(self.get_any_status(path)?)
    }

    pub fn post_json(&self, path: &str, body: Vec<u8>) -> Result<Response> {
        let body = Body {
            bytes: Bytes::from(body),
            content_type: JSON,
        };
        success(self.send(Method::POST, &self.resolve(path), Some(body), None)?)
    }

    pub fn get_anonymous(&self, url: &str) -> Result<Response> {
        self.send_once(self.bounded(Method::GET, url, None), &Method::GET, url)
    }

    pub fn post_form_anonymous(&self, url: &str, form: &[(&str, &str)]) -> Result<Response> {
        let encoded = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let body = Body {
            bytes: Bytes::from(encoded),
            content_type: FORM,
        };
        self.send_once(
            self.bounded(Method::POST, url, Some(&body)),
            &Method::POST,
            url,
        )
    }

    pub fn post_json_anonymous(&self, url: &str, body: Vec<u8>) -> Result<Response> {
        let body = Body {
            bytes: Bytes::from(body),
            content_type: JSON,
        };
        self.send_once(
            self.bounded(Method::POST, url, Some(&body)),
            &Method::POST,
            url,
        )
    }

    pub fn get_event_stream(&self, path: &str) -> Result<Response> {
        success(self.send(Method::GET, &self.resolve(path), None, Some(EVENT_STREAM))?)
    }

    fn send(
        &self,
        method: Method,
        url: &str,
        body: Option<Body>,
        accept: Option<&'static str>,
    ) -> Result<Response> {
        let mut retried = false;
        loop {
            let (header, generation) = self.fresh_header()?;
            let mut request = self.request(method.clone(), url, body.as_ref());
            if let Some(accept) = accept {
                request = request.header(ACCEPT, HeaderValue::from_static(accept));
            }
            if let Some(header) = header {
                request = request.header(AUTHORIZATION, header);
            }
            let response = self.send_once(request, &method, url)?;
            if !self.should_retry(&response, generation, &mut retried)? {
                return Ok(response);
            }
        }
    }

    fn fresh_header(&self) -> Result<(Option<HeaderValue>, u64)> {
        let Some(credential) = &self.credential else {
            return Ok((None, 0));
        };
        let (header, generation) = credential.header();
        let now = SystemTime::now();
        if !credential.needs_refresh(now) {
            return Ok((Some(header), generation));
        }
        match credential.refresh_after(self, generation) {
            Ok(()) => {
                let (header, generation) = credential.header();
                Ok((Some(header), generation))
            }
            Err(Error::SessionExpired) => Err(Error::SessionExpired),
            Err(_) if credential.is_unexpired(now) => Ok((Some(header), generation)),
            Err(e) => Err(e),
        }
    }

    fn should_retry(
        &self,
        response: &Response,
        generation: u64,
        retried: &mut bool,
    ) -> Result<bool> {
        let Some(credential) = &self.credential else {
            return Ok(false);
        };
        if *retried || response.status() != StatusCode::UNAUTHORIZED || !credential.is_refreshable()
        {
            return Ok(false);
        }
        *retried = true;
        credential.refresh_after(self, generation)?;
        Ok(true)
    }

    fn bounded(&self, method: Method, url: &str, body: Option<&Body>) -> RequestBuilder {
        self.request(method, url, body)
            .timeout(self.bounded_timeout)
    }

    fn request(&self, method: Method, url: &str, body: Option<&Body>) -> RequestBuilder {
        let request = self.inner.request(method, url);
        match body {
            Some(body) => request
                .header(CONTENT_TYPE, HeaderValue::from_static(body.content_type))
                .body(body.bytes.clone()),
            None => request,
        }
    }

    fn send_once(&self, request: RequestBuilder, method: &Method, url: &str) -> Result<Response> {
        debug!(target: HTTP_TRACE, "-> {method} {url}");
        let resp = request.send()?;
        if log_enabled!(target: HTTP_TRACE, Level::Debug) {
            let status = resp.status();
            let headers = resp.headers();
            let ct = header_str(headers.get(CONTENT_TYPE)).unwrap_or("(none)");
            let cl = header_str(headers.get(CONTENT_LENGTH)).unwrap_or("?");
            let ce = header_str(headers.get(CONTENT_ENCODING)).unwrap_or("identity");
            debug!(
                target: HTTP_TRACE,
                "<- {method} {url} status={status} content-type={ct} content-length={cl} content-encoding={ce}"
            );
        }
        Ok(resp)
    }
}

pub fn is_absolute(endpoint: &str) -> bool {
    endpoint.starts_with("https://") || endpoint.starts_with("http://")
}

pub fn success(resp: Response) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else {
        Err(status.into())
    }
}

pub(super) fn header_str(value: Option<&HeaderValue>) -> Option<&str> {
    value.and_then(|v| v.to_str().ok())
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/http/test_support.rs"]
mod test_support;

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/http/tests.rs"]
mod tests;
