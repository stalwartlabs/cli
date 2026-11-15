/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod sieve;
pub mod textarea;
pub mod units;

use crate::schema::{FieldType, NumberFormat, StringFormat};
use crate::tui::form::model::SECRET_MASK;
use crate::tui::theme::Theme;
use crate::tui::widgets::input::Input;
use crate::view::Color;
use crate::view::time::UtcTimestamp;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::{Number, Value};
use textarea::TextArea;
use units::Quantity;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text {
        format: StringFormat,
        nullable: bool,
    },
    Number {
        format: NumberFormat,
        nullable: bool,
    },
    DateTime {
        nullable: bool,
    },
}

impl Kind {
    pub fn of_field(typ: &FieldType) -> Option<Self> {
        match typ {
            FieldType::String {
                format, nullable, ..
            } => Some(Kind::Text {
                format: *format,
                nullable: *nullable,
            }),
            FieldType::Number {
                format, nullable, ..
            } => Some(Kind::Number {
                format: *format,
                nullable: *nullable,
            }),
            FieldType::UtcDateTime { nullable } => Some(Kind::DateTime {
                nullable: *nullable,
            }),
            FieldType::BlobId => Some(Kind::Text {
                format: StringFormat::Text,
                nullable: false,
            }),
            _ => None,
        }
    }

    pub fn placeholder(self) -> Option<&'static str> {
        match self {
            Kind::Text { format, .. } => match format {
                StringFormat::IpAddress => Some("192.0.2.1 or 2001:db8::1"),
                StringFormat::IpNetwork => Some("192.0.2.0/24"),
                StringFormat::SocketAddress => Some("0.0.0.0:25"),
                StringFormat::EmailAddress => Some("user@example.com"),
                StringFormat::Uri => Some("https://example.com"),
                StringFormat::Color => Some("#db2d54"),
                _ => None,
            },
            Kind::DateTime { .. } => Some("YYYY-MM-DDTHH:MM:SSZ, t sets now"),
            Kind::Number { .. } => None,
        }
    }

    fn nullable(self) -> bool {
        match self {
            Kind::Text { nullable, .. }
            | Kind::Number { nullable, .. }
            | Kind::DateTime { nullable } => nullable,
        }
    }

    fn is_secret(self) -> bool {
        matches!(
            self,
            Kind::Text {
                format: StringFormat::Secret | StringFormat::SecretText,
                ..
            }
        )
    }

    fn quantity(self) -> Option<Quantity> {
        match self {
            Kind::Number {
                format: NumberFormat::Size,
                ..
            } => Some(Quantity::Size),
            Kind::Number {
                format: NumberFormat::Duration,
                ..
            } => Some(Quantity::Duration),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
enum Body {
    Line(Input),
    Area(TextArea),
}

#[derive(Debug, Clone)]
pub struct Inline {
    kind: Kind,
    body: Body,
    unit: usize,
    revealed: bool,
    original_mask: bool,
}

impl Inline {
    pub fn open(kind: Kind, value: Option<&Value>) -> Self {
        let original_mask =
            kind.is_secret() && matches!(value, Some(Value::String(s)) if s == SECRET_MASK);
        let mut unit = 0;
        let text = match (kind.quantity(), value) {
            (_, _) if original_mask => String::new(),
            (Some(q), None | Some(Value::Null)) => {
                unit = q.empty_unit();
                String::new()
            }
            (Some(q), Some(Value::Number(n))) => {
                let raw = n.as_u64().unwrap_or_default();
                unit = q.best_unit(raw);
                let mut out = String::new();
                q.write_amount(raw, unit, &mut out);
                out
            }
            (_, Some(Value::String(s))) => s.clone(),
            (_, Some(Value::Number(n))) => n.to_string(),
            _ => String::new(),
        };
        let body = match kind {
            Kind::Text { format, .. } if format.is_multiline() => {
                Body::Area(TextArea::new(&text).at_top().masked(kind.is_secret()))
            }
            _ if kind.is_secret() => Body::Line(Input::masked(&text)),
            _ => Body::Line(Input::new(&text)),
        };
        Self {
            kind,
            body,
            unit,
            revealed: false,
            original_mask,
        }
    }

    pub fn is_multiline(&self) -> bool {
        matches!(self.body, Body::Area(_))
    }

    pub fn text(&self) -> String {
        match &self.body {
            Body::Line(input) => input.value().to_string(),
            Body::Area(area) => area.text(),
        }
    }

    pub fn handle(&mut self, key: KeyEvent) -> bool {
        if let Some(quantity) = self.kind.quantity() {
            let count = quantity.units().len();
            match key.code {
                KeyCode::Tab => {
                    self.unit = (self.unit + 1) % count;
                    return true;
                }
                KeyCode::BackTab => {
                    self.unit = (self.unit + count - 1) % count;
                    return true;
                }
                _ => {}
            }
        }
        if self.kind.is_secret()
            && key.code == KeyCode::Char('r')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.toggle_reveal();
            return true;
        }
        match &mut self.body {
            Body::Line(input) => input.handle(key),
            Body::Area(area) => area.handle(key),
        }
    }

    pub fn paste(&mut self, text: &str) {
        match &mut self.body {
            Body::Line(input) => input.paste(text.lines().next().unwrap_or_default()),
            Body::Area(area) => area.paste(text),
        }
    }

    pub fn toggle_reveal(&mut self) {
        self.revealed = !self.revealed;
        match &mut self.body {
            Body::Line(input) => {
                *input = if self.revealed {
                    Input::new(input.value())
                } else {
                    Input::masked(input.value())
                };
            }
            Body::Area(area) => area.set_masked(!self.revealed),
        }
    }

    pub fn commit(&self) -> Result<Value, String> {
        let text = self.text();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            if self.original_mask {
                return Ok(Value::String(SECRET_MASK.into()));
            }
            if self.kind.nullable() {
                return Ok(Value::Null);
            }
        }
        match self.kind {
            Kind::Text { format, .. } => {
                if format == StringFormat::Color
                    && !trimmed.is_empty()
                    && Color::parse(trimmed) == Color::Default
                {
                    return Err("Expected a colour such as #db2d54".into());
                }
                Ok(Value::String(if format.is_multiline() {
                    text
                } else {
                    trimmed.to_string()
                }))
            }
            Kind::Number { format, .. } => match self.kind.quantity() {
                Some(q) => q
                    .parse(trimmed, self.unit)
                    .map(Value::from)
                    .map_err(|e| e.message().to_string()),
                None => parse_number(trimmed, format),
            },
            Kind::DateTime { .. } => {
                if UtcTimestamp::parse(trimmed).is_some() {
                    Ok(Value::String(trimmed.to_string()))
                } else {
                    Err("Expected a date such as 2026-01-31T12:00:00Z".into())
                }
            }
        }
    }

    pub fn lines<'a>(&'a mut self, theme: &Theme, width: u16, height: usize) -> Vec<Line<'a>> {
        let unit = self
            .kind
            .quantity()
            .and_then(|q| q.units().get(self.unit))
            .map(|u| u.label);
        let swatch = match (self.kind, &self.body) {
            (
                Kind::Text {
                    format: StringFormat::Color,
                    ..
                },
                Body::Line(input),
            ) => match Color::parse(input.value()) {
                Color::Rgb(r, g, b) => Some(ratatui::style::Color::Rgb(r, g, b)),
                _ => None,
            },
            _ => None,
        };
        match &mut self.body {
            Body::Line(input) => {
                let field_width = width.saturating_sub(if unit.is_some() { 8 } else { 0 });
                let empty = input.is_empty();
                let mut line = input.line(theme, field_width, true);
                if empty && let Some(placeholder) = self.kind.placeholder() {
                    line.spans.push(Span::styled(placeholder, theme.muted()));
                }
                if let Some(unit) = unit {
                    line.spans.push(Span::raw(" "));
                    line.spans.push(Span::styled(unit, theme.title()));
                    line.spans.push(Span::styled(" ▾ tab", theme.muted()));
                }
                if let Some(color) = swatch {
                    line.spans.push(Span::styled(" ██", Style::new().fg(color)));
                }
                vec![line]
            }
            Body::Area(area) => area.lines(theme, height, true).collect(),
        }
    }
}

fn parse_number(text: &str, format: NumberFormat) -> Result<Value, String> {
    match format {
        NumberFormat::UnsignedInteger | NumberFormat::Size | NumberFormat::Duration => text
            .parse::<u64>()
            .map(Value::from)
            .map_err(|_| "Expected a whole number, 0 or more".into()),
        NumberFormat::Integer => text
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| "Expected a whole number".into()),
        NumberFormat::Float | NumberFormat::Unknown => text
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| "Expected a number".into()),
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/form/editor/tests.rs"]
mod tests;
