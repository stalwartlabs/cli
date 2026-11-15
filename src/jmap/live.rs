/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::jmap::sse::{EventStream, StreamClient};
use crate::view::time::UtcTimestamp;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::fmt;
use std::io::BufRead;
use std::marker::PhantomData;

const METRICS_PATH: &str = "/api/live/metrics?";
const TRACING_PATH: &str = "/api/live/tracing?";
const DELIVERY_PATH: &str = "/api/live/delivery/";
const MAX_LIST_ITEMS: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum MetricKind {
    Counter,
    Gauge,
    Histogram,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct LiveSample {
    pub metric: Box<str>,
    #[serde(rename = "@type")]
    pub kind: MetricKind,
    pub count: f64,
    #[serde(default)]
    pub sum: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceEvent {
    pub event: Box<str>,
    #[serde(deserialize_with = "timestamp")]
    pub timestamp: UtcTimestamp,
    #[serde(default, deserialize_with = "index_map")]
    pub key_values: Vec<KeyValue>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct KeyValue {
    pub key: Box<str>,
    pub value: TraceValue,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "@type")]
pub enum TraceValue {
    String {
        value: Box<str>,
    },
    UnsignedInt {
        value: u64,
    },
    Integer {
        value: i64,
    },
    Boolean {
        value: bool,
    },
    Float {
        value: Option<f64>,
    },
    #[serde(rename = "UTCDateTime")]
    UtcDateTime {
        #[serde(deserialize_with = "timestamp")]
        value: UtcTimestamp,
    },
    Duration {
        value: u64,
    },
    IpAddr {
        value: Box<str>,
    },
    List {
        #[serde(deserialize_with = "index_map")]
        value: Vec<TraceValue>,
    },
    Event {
        event: Box<str>,
        #[serde(default, deserialize_with = "index_map")]
        value: Vec<KeyValue>,
    },
    #[serde(other)]
    Null,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TraceFrame {
    Ping,
    Events(Vec<TraceEvent>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeliveryStage {
    pub kind: StageKind,
    pub fields: Map<String, Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Pending,
    Ok,
    Warn,
    Error,
}

pub struct MetricsStream<R = std::io::BufReader<reqwest::blocking::Response>> {
    events: EventStream<R>,
}

pub struct TracingStream<R = std::io::BufReader<reqwest::blocking::Response>> {
    events: EventStream<R>,
}

pub struct DeliveryStream<R = std::io::BufReader<reqwest::blocking::Response>> {
    events: EventStream<R>,
    pending: std::vec::IntoIter<DeliveryStage>,
    done: bool,
}

macro_rules! stage_kinds {
    ($($variant:ident => $wire:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum StageKind {
            $($variant,)*
            Completed,
            Unknown,
        }

        impl StageKind {
            pub fn parse(wire: &str) -> Self {
                hashify::map!(
                    wire.as_bytes(),
                    StageKind,
                    $($wire => StageKind::$variant,)*
                    "completed" => StageKind::Completed,
                )
                .copied()
                .unwrap_or(StageKind::Unknown)
            }

            pub fn as_str(self) -> &'static str {
                match self {
                    $(StageKind::$variant => $wire,)*
                    StageKind::Completed => "completed",
                    StageKind::Unknown => "unknown",
                }
            }
        }
    };
}

stage_kinds! {
    MxLookupStart => "mxLookupStart",
    MxLookupSuccess => "mxLookupSuccess",
    MxLookupError => "mxLookupError",
    MtaStsFetchStart => "mtaStsFetchStart",
    MtaStsFetchSuccess => "mtaStsFetchSuccess",
    MtaStsFetchError => "mtaStsFetchError",
    MtaStsNotFound => "mtaStsNotFound",
    TlsRptLookupStart => "tlsRptLookupStart",
    TlsRptLookupSuccess => "tlsRptLookupSuccess",
    TlsRptLookupError => "tlsRptLookupError",
    TlsRptNotFound => "tlsRptNotFound",
    DeliveryAttemptStart => "deliveryAttemptStart",
    MtaStsVerifySuccess => "mtaStsVerifySuccess",
    MtaStsVerifyError => "mtaStsVerifyError",
    TlsaLookupStart => "tlsaLookupStart",
    TlsaLookupSuccess => "tlsaLookupSuccess",
    TlsaNotFound => "tlsaNotFound",
    TlsaLookupError => "tlsaLookupError",
    IpLookupStart => "ipLookupStart",
    IpLookupSuccess => "ipLookupSuccess",
    IpLookupError => "ipLookupError",
    ConnectionStart => "connectionStart",
    ConnectionSuccess => "connectionSuccess",
    ConnectionError => "connectionError",
    ReadGreetingStart => "readGreetingStart",
    ReadGreetingSuccess => "readGreetingSuccess",
    ReadGreetingError => "readGreetingError",
    EhloStart => "ehloStart",
    EhloSuccess => "ehloSuccess",
    EhloError => "ehloError",
    StartTlsStart => "startTlsStart",
    StartTlsSuccess => "startTlsSuccess",
    StartTlsError => "startTlsError",
    DaneVerifySuccess => "daneVerifySuccess",
    DaneVerifyError => "daneVerifyError",
    MailFromStart => "mailFromStart",
    MailFromSuccess => "mailFromSuccess",
    MailFromError => "mailFromError",
    RcptToStart => "rcptToStart",
    RcptToSuccess => "rcptToSuccess",
    RcptToError => "rcptToError",
    QuitStart => "quitStart",
    QuitCompleted => "quitCompleted",
}

impl StageKind {
    pub fn severity(self) -> Severity {
        let wire = self.as_str();
        if wire.ends_with("Error") {
            Severity::Error
        } else if wire.ends_with("NotFound") {
            Severity::Warn
        } else if wire.ends_with("Start") {
            Severity::Pending
        } else {
            Severity::Ok
        }
    }
}

impl DeliveryStage {
    pub fn elapsed_ms(&self) -> Option<u64> {
        self.fields.get("elapsed").and_then(Value::as_u64)
    }

    pub fn reason(&self) -> Option<&str> {
        self.text("reason")
    }

    pub fn hostname(&self) -> Option<&str> {
        self.text("hostname")
    }

    pub fn remote_ip(&self) -> Option<&str> {
        self.text("remoteIp")
    }

    pub fn text(&self, field: &str) -> Option<&str> {
        self.fields.get(field).and_then(Value::as_str)
    }

    fn from_value(value: Value) -> Result<Self> {
        let Value::Object(mut fields) = value else {
            return Err(Error::UnexpectedResponse(
                "delivery stage is not an object".into(),
            ));
        };
        let kind = match fields.remove("type") {
            Some(Value::String(wire)) => StageKind::parse(&wire),
            _ => {
                return Err(Error::UnexpectedResponse(
                    "delivery stage without a type".into(),
                ));
            }
        };
        Ok(DeliveryStage { kind, fields })
    }
}

impl StreamClient {
    pub fn live_metrics(&self, interval_s: u32, metrics: &[&str]) -> Result<MetricsStream> {
        let mut path = String::with_capacity(METRICS_PATH.len() + 32 + metrics.len() * 24);
        path.push_str(METRICS_PATH);
        let interval = interval_s.max(1).to_string();
        let joined = metrics.join(",");
        let mut query = form_urlencoded::Serializer::for_suffix(path, METRICS_PATH.len());
        query.append_pair("interval", &interval);
        if !joined.is_empty() {
            query.append_pair("metrics", &joined);
        }
        Ok(MetricsStream::new(self.open(&query.finish())?))
    }

    pub fn live_tracing(
        &self,
        filter: Option<&str>,
        keys: &[(&str, &str)],
    ) -> Result<TracingStream> {
        let mut query =
            form_urlencoded::Serializer::for_suffix(String::from(TRACING_PATH), TRACING_PATH.len());
        if let Some(filter) = filter.map(str::trim).filter(|f| !f.is_empty()) {
            query.append_pair("filter", filter);
        }
        query.extend_pairs(
            keys.iter()
                .filter(|(k, v)| !v.is_empty() && *k != "filter" && *k != "token"),
        );
        Ok(TracingStream::new(self.open(&query.finish())?))
    }

    pub fn live_delivery(&self, target: &str) -> Result<DeliveryStream> {
        let mut path = String::with_capacity(DELIVERY_PATH.len() + target.len() * 3);
        path.push_str(DELIVERY_PATH);
        path.extend(utf8_percent_encode(target.trim(), NON_ALPHANUMERIC));
        Ok(DeliveryStream::new(self.open(&path)?))
    }
}

impl<R: BufRead> MetricsStream<R> {
    pub fn new(events: EventStream<R>) -> Self {
        MetricsStream { events }
    }

    pub fn next_frame(&mut self) -> Result<Option<Vec<LiveSample>>> {
        while let Some(event) = self.events.next_event()? {
            if event.name == "metrics" {
                let mut samples = parse_batch::<LiveSample>(event.data)?;
                samples.retain(|s| s.kind != MetricKind::Unknown);
                return Ok(Some(samples));
            }
        }
        Ok(None)
    }
}

impl<R: BufRead> TracingStream<R> {
    pub fn new(events: EventStream<R>) -> Self {
        TracingStream { events }
    }

    pub fn next_frame(&mut self) -> Result<Option<TraceFrame>> {
        while let Some(event) = self.events.next_event()? {
            match event.name {
                "ping" => return Ok(Some(TraceFrame::Ping)),
                "trace" => return Ok(Some(TraceFrame::Events(parse_batch(event.data)?))),
                _ => {}
            }
        }
        Ok(None)
    }
}

impl<R: BufRead> DeliveryStream<R> {
    pub fn new(events: EventStream<R>) -> Self {
        DeliveryStream {
            events,
            pending: Vec::new().into_iter(),
            done: false,
        }
    }

    pub fn next_stage(&mut self) -> Result<Option<DeliveryStage>> {
        loop {
            if let Some(stage) = self.pending.next() {
                if stage.kind == StageKind::Completed {
                    self.done = true;
                    self.pending = Vec::new().into_iter();
                    return Ok(None);
                }
                return Ok(Some(stage));
            }
            if self.done {
                return Ok(None);
            }
            let Some(event) = self.events.next_event()? else {
                return Ok(None);
            };
            if event.name != "event" {
                continue;
            }
            let stages = parse_batch::<Value>(event.data)?
                .into_iter()
                .map(DeliveryStage::from_value)
                .collect::<Result<Vec<_>>>()?;
            self.pending = stages.into_iter();
        }
    }
}

fn parse_batch<T: DeserializeOwned>(data: &str) -> Result<Vec<T>> {
    let parsed = if data.trim_start().starts_with('[') {
        serde_json::from_str(data)
    } else {
        serde_json::from_str(data).map(|item| vec![item])
    };
    parsed.map_err(|e| Error::UnexpectedResponse(format!("invalid event data: {e}")))
}

fn timestamp<'de, D: Deserializer<'de>>(deserializer: D) -> Result<UtcTimestamp, D::Error> {
    let raw = Cow::<str>::deserialize(deserializer)?;
    UtcTimestamp::parse(&raw).ok_or_else(|| de::Error::custom("invalid timestamp"))
}

fn index_map<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    deserializer.deserialize_any(IndexMapVisitor(PhantomData))
}

struct IndexMapVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for IndexMapVisitor<T> {
    type Value = Vec<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array or an object keyed by index")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(64));
        while let Some(item) = seq.next_element()? {
            if items.len() == MAX_LIST_ITEMS {
                return Err(de::Error::custom("list too long"));
            }
            items.push(item);
        }
        Ok(items)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Vec<T>, A::Error> {
        let mut items: Vec<(u32, T)> = Vec::with_capacity(map.size_hint().unwrap_or(0).min(64));
        while let Some((index, item)) = map.next_entry::<Cow<'de, str>, T>()? {
            if items.len() == MAX_LIST_ITEMS {
                return Err(de::Error::custom("list too long"));
            }
            let index = index
                .parse()
                .map_err(|_| de::Error::custom("list key is not an index"))?;
            items.push((index, item));
        }
        items.sort_unstable_by_key(|(index, _)| *index);
        Ok(items.into_iter().map(|(_, item)| item).collect())
    }

    fn visit_unit<E: de::Error>(self) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/live/tests.rs"]
mod tests;
