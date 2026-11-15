/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::form::model::{OTP_CODE, OTP_URL, SECRET_MASK};
use crate::tui::theme::Theme;
use crate::tui::widgets::input::Input;
use encodify::base32::STANDARD_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use qrcode::{EcLevel, QrCode};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::{Map, Value};
use sha1::Sha1;
use std::time::{SystemTime, UNIX_EPOCH};

const SECRET_BYTES: usize = 20;
const ISSUER: &str = "Stalwart";
const LABEL: &str = "account";
const PERIOD_SECS: u64 = 30;
const DIGITS: u32 = 6;
const WINDOW: u64 = 1;
const IMAGE_URL: &str = "https%3A%2F%2Fstalw.art%2Fimg%2Ffavicon-32x32.png";
pub const OTP_AUTH: &str = "x:OtpAuth";
const ENTER_CODE: &str = "Enter the code shown in your authenticator.";
const WRONG_CODE: &str = "That code is incorrect. Make sure your authenticator clock is in sync.";

#[derive(Debug)]
pub enum OtpEditor {
    Enrol {
        setup: Box<TotpSetup>,
        qr: Vec<String>,
        secret: String,
        code: Input,
        error: Option<&'static str>,
    },
    Confirm {
        code: Input,
    },
}

impl OtpEditor {
    pub fn open(
        value: Option<&Value>,
        pending: &mut Option<Box<TotpSetup>>,
    ) -> Result<Self, getrandom::Error> {
        if is_configured(value) {
            let code = value
                .and_then(|v| v.get(OTP_CODE)?.as_str())
                .filter(|code| *code != SECRET_MASK)
                .unwrap_or_default();
            return Ok(OtpEditor::Confirm {
                code: Input::new(code),
            });
        }
        let setup = match pending {
            Some(setup) => setup.clone(),
            None => pending.insert(Box::new(TotpSetup::generate()?)).clone(),
        };
        Ok(OtpEditor::Enrol {
            qr: setup.qr_rows(),
            secret: setup.grouped_secret(),
            setup,
            code: Input::default(),
            error: None,
        })
    }

    pub fn key(&mut self, key: KeyEvent, current: Option<&Value>) -> Option<Value> {
        match self {
            OtpEditor::Confirm { code }
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('d') =>
            {
                Some(with(current, Value::Null, code.value()))
            }
            OtpEditor::Confirm { code } | OtpEditor::Enrol { code, .. } => {
                code.handle(key);
                None
            }
        }
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            OtpEditor::Enrol { setup, .. } => Some(setup.url()),
            OtpEditor::Confirm { .. } => None,
        }
    }

    pub fn paste(&mut self, text: &str) {
        let (OtpEditor::Confirm { code } | OtpEditor::Enrol { code, .. }) = self;
        code.paste(text);
    }

    pub fn commit(&mut self, current: Option<&Value>) -> Option<Value> {
        match self {
            OtpEditor::Confirm { code } => {
                let url = current
                    .and_then(|v| v.get(OTP_URL))
                    .cloned()
                    .unwrap_or(Value::Null);
                Some(with(current, url, code.value().trim()))
            }
            OtpEditor::Enrol {
                setup, code, error, ..
            } => {
                let text = code.value().trim();
                if text.is_empty() {
                    *error = Some(ENTER_CODE);
                    None
                } else if setup.verify(text) {
                    Some(with(current, Value::String(setup.url().into()), text))
                } else {
                    *error = Some(WRONG_CODE);
                    None
                }
            }
        }
    }

    pub fn lines<'a>(&'a mut self, theme: &Theme, width: u16) -> Vec<Line<'a>> {
        let label = |text: &'static str| Span::styled(text, theme.muted());
        match self {
            OtpEditor::Confirm { code } => vec![
                Line::raw("Two-factor authentication is enabled."),
                Line::styled(
                    "Enter your current code to authorise any change to this account.",
                    theme.muted(),
                ),
                labelled(label("Code  "), code.line(theme, width, true)),
                Line::styled("⏎ done · ^d disable two-factor · esc cancel", theme.muted()),
            ],
            OtpEditor::Enrol {
                setup,
                qr,
                secret,
                code,
                error,
            } => {
                let mut lines = Vec::with_capacity(qr.len() + 8);
                lines.push(Line::raw(
                    "Scan the QR code below with an authenticator app,",
                ));
                lines.push(Line::raw("then enter the 6-digit code it shows."));
                lines.push(labelled(label("Code    "), code.line(theme, width, true)));
                if let Some(error) = error {
                    lines.push(Line::styled(*error, Style::new().fg(theme.err)));
                }
                lines.push(Line::styled(
                    "⏎ confirm · ^y copy URL · esc cancel",
                    theme.muted(),
                ));
                lines.push(Line::from(vec![
                    label("Secret  "),
                    Span::raw(secret.as_str()),
                ]));
                lines.push(Line::from(vec![label("URL     "), Span::raw(setup.url())]));
                lines.extend(qr.iter().map(|row| Line::raw(row.as_str())));
                lines
            }
        }
    }
}

fn labelled<'a>(label: Span<'a>, mut line: Line<'a>) -> Line<'a> {
    line.spans.insert(0, label);
    line
}

pub fn is_configured(value: Option<&Value>) -> bool {
    value
        .and_then(|v| v.get(OTP_URL)?.as_str())
        .is_some_and(|url| !url.is_empty())
}

pub fn status(value: Option<&Value>) -> &'static str {
    if is_configured(value) {
        "enabled"
    } else {
        "not enabled"
    }
}

fn with(current: Option<&Value>, url: Value, code: &str) -> Value {
    let mut map = match current {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    };
    map.insert(OTP_URL.into(), url);
    map.insert(OTP_CODE.into(), Value::String(code.into()));
    Value::Object(map)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TotpSetup {
    key: [u8; SECRET_BYTES],
    secret: Box<str>,
    url: Box<str>,
}

impl TotpSetup {
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0u8; SECRET_BYTES];
        getrandom::fill(&mut bytes)?;
        Ok(Self::from_secret(bytes))
    }

    fn from_secret(key: [u8; SECRET_BYTES]) -> Self {
        let secret = STANDARD_NO_PAD.encode(key);
        let url = format!(
            "otpauth://totp/{ISSUER}:{LABEL}?issuer={ISSUER}&secret={secret}&algorithm=SHA1&digits=6&period=30&image={IMAGE_URL}"
        );
        Self {
            key,
            secret: secret.into_boxed_str(),
            url: url.into_boxed_str(),
        }
    }

    pub fn verify(&self, code: &str) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.verify_at(code, now)
    }

    fn verify_at(&self, code: &str, unix_secs: u64) -> bool {
        let code = code.trim();
        let Ok(wanted) = code.parse::<u32>() else {
            return false;
        };
        if !is_code(code) {
            return false;
        }
        let counter = unix_secs / PERIOD_SECS;
        (counter.saturating_sub(WINDOW)..=counter.saturating_add(WINDOW))
            .any(|c| hotp(&self.key, c, DIGITS) == Some(wanted))
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn grouped_secret(&self) -> String {
        let mut out = String::with_capacity(self.secret.len() + self.secret.len() / 4);
        for (i, c) in self.secret.chars().enumerate() {
            if i > 0 && i % 4 == 0 {
                out.push(' ');
            }
            out.push(c);
        }
        out
    }

    pub fn qr_rows(&self) -> Vec<String> {
        qr_half_blocks(&self.url)
    }
}

pub fn qr_half_blocks(data: &str) -> Vec<String> {
    let Ok(code) = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::L) else {
        return Vec::new();
    };
    let width = code.width();
    let colors = code.to_colors();
    let dark = |x: usize, y: usize| {
        x < width
            && y < width
            && colors
                .get(y * width + x)
                .is_some_and(|c| *c == qrcode::Color::Dark)
    };
    let quiet = 1;
    let span = width + 2 * quiet;
    let mut rows = Vec::with_capacity(span.div_ceil(2));
    for pair in (0..span).step_by(2) {
        let mut row = String::with_capacity(span * 3);
        for col in 0..span {
            let x = col.wrapping_sub(quiet);
            let top = dark(x, pair.wrapping_sub(quiet));
            let bottom = dark(x, (pair + 1).wrapping_sub(quiet));
            row.push(match (top, bottom) {
                (false, false) => '█',
                (true, false) => '▄',
                (false, true) => '▀',
                (true, true) => ' ',
            });
        }
        rows.push(row);
    }
    rows
}

fn hotp(key: &[u8], counter: u64, digits: u32) -> Option<u32> {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest.last()? & 0x0f);
    let window: [u8; 4] = digest.get(offset..offset + 4)?.try_into().ok()?;
    let binary = u32::from_be_bytes(window) & 0x7fff_ffff;
    Some(binary % 10u32.pow(digits))
}

pub fn is_code(text: &str) -> bool {
    let text = text.trim();
    text.len() == 6 && text.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/form/otp/tests.rs"]
mod tests;
