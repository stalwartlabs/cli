/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::jmap::credential::Credential;
use crate::jmap::http::{ConnectOptions, HttpClient, TlsVerify};
use reqwest::blocking::Response;
use std::io::{BufRead, BufReader, Read};
use std::sync::Arc;

const MAX_FRAME_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_EVENT: &str = "message";

pub struct StreamClient {
    http: HttpClient,
}

pub struct EventStream<R = BufReader<Response>> {
    reader: R,
    line: String,
    event: String,
    data: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SseEvent<'a> {
    pub name: &'a str,
    pub data: &'a str,
}

#[derive(Clone, Copy)]
enum Field {
    Event,
    Data,
}

impl StreamClient {
    pub fn new(base_url: &str, credential: Arc<Credential>, tls: TlsVerify) -> Result<Self> {
        let http = HttpClient::streaming(&ConnectOptions {
            url: base_url.to_string(),
            credential,
            tls,
        })?;
        Ok(StreamClient { http })
    }

    pub fn open(&self, path_and_query: &str) -> Result<EventStream> {
        let response = self.http.get_event_stream(path_and_query)?;
        Ok(EventStream::new(BufReader::new(response)))
    }
}

impl<R: BufRead> EventStream<R> {
    pub fn new(reader: R) -> Self {
        EventStream {
            reader,
            line: String::new(),
            event: String::new(),
            data: String::new(),
        }
    }

    pub fn next_event(&mut self) -> Result<Option<SseEvent<'_>>> {
        self.event.clear();
        self.data.clear();
        let mut has_data = false;
        loop {
            self.line.clear();
            let budget = MAX_FRAME_BYTES.saturating_sub(self.data.len() as u64);
            let read = (&mut self.reader).take(budget).read_line(&mut self.line)?;
            if read == 0 {
                return if budget == 0 {
                    Err(frame_too_large())
                } else {
                    Ok(None)
                };
            }
            if !self.line.ends_with('\n') && read as u64 == budget {
                return Err(frame_too_large());
            }
            let line = self.line.strip_suffix('\n').unwrap_or(&self.line);
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                if has_data {
                    break;
                }
                self.event.clear();
                continue;
            }
            let (name, value) = match line.split_once(':') {
                Some(("", _)) => continue,
                Some((name, value)) => (name, value.strip_prefix(' ').unwrap_or(value)),
                None => (line, ""),
            };
            match hashify::map!(name.as_bytes(), Field, "event" => Field::Event, "data" => Field::Data)
            {
                Some(Field::Event) => {
                    self.event.clear();
                    self.event.push_str(value);
                }
                Some(Field::Data) => {
                    if has_data {
                        self.data.push('\n');
                    }
                    self.data.push_str(value);
                    has_data = true;
                }
                None => {}
            }
        }
        Ok(Some(SseEvent {
            name: if self.event.is_empty() {
                DEFAULT_EVENT
            } else {
                &self.event
            },
            data: &self.data,
        }))
    }
}

#[cold]
fn frame_too_large() -> Error {
    Error::UnexpectedResponse("event stream frame too large".into())
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/sse/tests.rs"]
mod tests;
