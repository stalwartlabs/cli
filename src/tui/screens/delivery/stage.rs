/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::live::{DeliveryStage, StageKind};
use serde_json::Value;
use std::borrow::Cow;
use std::fmt::Write;

const INLINE_IPS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Mx,
    MtaStsFetch,
    TlsRpt,
    MtaStsVerify,
    Tlsa,
    Ip,
    Connection,
    Greeting,
    Ehlo,
    StartTls,
    Dane,
    MailFrom,
    RcptTo,
    Quit,
    Attempt,
    Other,
}

impl Phase {
    pub const COUNT: usize = 16;

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn of(kind: StageKind) -> Self {
        use StageKind::*;
        match kind {
            MxLookupStart | MxLookupSuccess | MxLookupError => Phase::Mx,
            MtaStsFetchStart | MtaStsFetchSuccess | MtaStsFetchError | MtaStsNotFound => {
                Phase::MtaStsFetch
            }
            TlsRptLookupStart | TlsRptLookupSuccess | TlsRptLookupError | TlsRptNotFound => {
                Phase::TlsRpt
            }
            MtaStsVerifySuccess | MtaStsVerifyError => Phase::MtaStsVerify,
            TlsaLookupStart | TlsaLookupSuccess | TlsaNotFound | TlsaLookupError => Phase::Tlsa,
            IpLookupStart | IpLookupSuccess | IpLookupError => Phase::Ip,
            ConnectionStart | ConnectionSuccess | ConnectionError => Phase::Connection,
            ReadGreetingStart | ReadGreetingSuccess | ReadGreetingError => Phase::Greeting,
            EhloStart | EhloSuccess | EhloError => Phase::Ehlo,
            StartTlsStart | StartTlsSuccess | StartTlsError => Phase::StartTls,
            DaneVerifySuccess | DaneVerifyError => Phase::Dane,
            MailFromStart | MailFromSuccess | MailFromError => Phase::MailFrom,
            RcptToStart | RcptToSuccess | RcptToError => Phase::RcptTo,
            QuitStart | QuitCompleted => Phase::Quit,
            DeliveryAttemptStart => Phase::Attempt,
            Completed | Unknown => Phase::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Phase::Mx => "MX Lookup",
            Phase::MtaStsFetch => "MTA-STS Policy Fetch",
            Phase::TlsRpt => "TLS-RPT Lookup",
            Phase::MtaStsVerify => "MTA-STS Verify",
            Phase::Tlsa => "TLSA / DANE Lookup",
            Phase::Ip => "IP Lookup",
            Phase::Connection => "TCP Connection",
            Phase::Greeting => "Read Greeting",
            Phase::Ehlo => "EHLO",
            Phase::StartTls => "STARTTLS",
            Phase::Dane => "DANE Verify",
            Phase::MailFrom => "MAIL FROM",
            Phase::RcptTo => "RCPT TO",
            Phase::Quit => "QUIT",
            Phase::Attempt => "Delivery Attempt",
            Phase::Other => "Stage",
        }
    }
}

const _: () = assert!(Phase::Other as usize + 1 == Phase::COUNT);

pub fn valid_target(target: &str) -> bool {
    let target = target.trim();
    match target.rsplit_once('@') {
        Some((local, domain)) => !local.is_empty() && domain.contains('.'),
        None => target.contains('.'),
    }
}

pub fn body(stage: &DeliveryStage) -> Option<Cow<'_, str>> {
    use StageKind::*;
    let text = match stage.kind {
        MxLookupStart => {
            return Some(
                format!("Looking up MX for {}", stage.text("domain").unwrap_or("?")).into(),
            );
        }
        MxLookupSuccess => {
            let count = array(stage, "mxs")
                .filter_map(|mx| mx.get("exchanges").and_then(Value::as_array))
                .map(Vec::len)
                .sum::<usize>();
            return Some(format!("Resolved {count} MX records").into());
        }
        MtaStsFetchSuccess => "Policy fetched successfully",
        MtaStsNotFound => "No MTA-STS policy published",
        TlsRptLookupSuccess => {
            return Some(format!("Found {} reporting URIs", array(stage, "rua").count()).into());
        }
        TlsRptNotFound => "No TLS-RPT record published",
        IpLookupSuccess => {
            let ips = array(stage, "remoteIps").filter_map(Value::as_str);
            let total = array(stage, "remoteIps").count();
            let mut out = String::new();
            for (index, ip) in ips.take(INLINE_IPS).enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(ip);
            }
            if total > INLINE_IPS {
                let _ = write!(out, " +{} more", total - INLINE_IPS);
            }
            return Some(out.into());
        }
        ConnectionStart => {
            return Some(format!("Connecting to {}", stage.remote_ip().unwrap_or("?")).into());
        }
        ConnectionSuccess => "Connected",
        TlsaLookupSuccess => "TLSA record found",
        TlsaNotFound => {
            return Some(
                stage
                    .reason()
                    .filter(|r| !r.is_empty())
                    .unwrap_or("No TLSA record found")
                    .into(),
            );
        }
        ReadGreetingSuccess => "Server greeting received",
        EhloSuccess => "EHLO accepted",
        StartTlsSuccess => "TLS negotiated",
        DaneVerifySuccess => "DANE verification passed",
        MtaStsVerifySuccess => "MTA-STS hostname verified",
        MailFromSuccess => "Sender accepted",
        RcptToSuccess => "Recipient accepted",
        QuitCompleted => "Session closed",
        _ => return stage.reason().map(Cow::Borrowed),
    };
    Some(Cow::Borrowed(text))
}

pub fn details(stage: &DeliveryStage, out: &mut Vec<String>) {
    match stage.kind {
        StageKind::MxLookupSuccess => {
            let mut mxs = array(stage, "mxs")
                .map(|mx| {
                    (
                        mx.get("preference").and_then(Value::as_u64).unwrap_or(0),
                        mx.get("exchanges").and_then(Value::as_array),
                    )
                })
                .collect::<Vec<_>>();
            mxs.sort_by_key(|(preference, _)| *preference);
            for (preference, exchanges) in mxs {
                for exchange in exchanges.into_iter().flatten().filter_map(Value::as_str) {
                    out.push(format!("{preference:>5}  {exchange}"));
                }
            }
        }
        StageKind::TlsRptLookupSuccess => {
            for rua in array(stage, "rua") {
                let target = rua
                    .get("email")
                    .or_else(|| rua.get("url"))
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let kind = rua.get("type").and_then(Value::as_str).unwrap_or("?");
                out.push(format!("{kind:>5}  {target}"));
            }
        }
        StageKind::MtaStsFetchSuccess => object_lines(stage.fields.get("policy"), out),
        StageKind::TlsaLookupSuccess => object_lines(stage.fields.get("record"), out),
        _ => {}
    }
}

fn object_lines(value: Option<&Value>, out: &mut Vec<String>) {
    let Some(Value::Object(map)) = value else {
        return;
    };
    for (key, value) in map {
        match value {
            Value::String(text) => out.push(format!("{key}: {text}")),
            other => out.push(format!("{key}: {other}")),
        }
    }
}

fn array<'a>(stage: &'a DeliveryStage, field: &str) -> impl Iterator<Item = &'a Value> + use<'a> {
    stage
        .fields
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}
