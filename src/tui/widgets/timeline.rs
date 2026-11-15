/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::live::{KeyValue, TraceEvent, TraceValue};
use crate::schema::{EnumVariant, Schema};
use crate::tui::theme::Theme;
use crate::tui::widgets::chrome::pad;
use crate::view::time::UtcTimestamp;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::fmt::Write;
use std::sync::Arc;

const EVENT_ENUM: &str = "EventType";
const KEY_ENUM: &str = "Key";
const INLINE_LIST_MAX: usize = 3;
const NESTED_DEPTH: usize = 2;
const INLINE_DEPTH: usize = 8;
const KEY_WIDTH: usize = 20;
const INDENT_WIDTH: usize = 4;
const KEY_GAP: &str = "  ";
const ID_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz792013";
const ERROR_MARKERS: [&str; 4] = ["error", "failed", "invalid", "reject"];
pub const EMPTY: &str = "No trace events to display.";

pub struct Labels {
    schema: Arc<Schema>,
    events: Vec<u32>,
    keys: Vec<u32>,
}

impl Labels {
    pub fn new(schema: Arc<Schema>) -> Self {
        let events = sorted_index(schema.enums.get(EVENT_ENUM));
        let keys = sorted_index(schema.enums.get(KEY_ENUM));
        Labels {
            schema,
            events,
            keys,
        }
    }

    pub fn key_variants(&self) -> &[EnumVariant] {
        self.schema.enums.get(KEY_ENUM).map_or(&[], Vec::as_slice)
    }

    pub fn event(&self, name: &str) -> Option<&EnumVariant> {
        find(self.schema.enums.get(EVENT_ENUM), &self.events, name)
    }

    pub fn event_label<'a>(&'a self, name: &'a str) -> &'a str {
        self.event(name).map_or(name, |v| v.label.as_str())
    }

    pub fn key_label<'a>(&'a self, name: &'a str) -> &'a str {
        find(self.schema.enums.get(KEY_ENUM), &self.keys, name).map_or(name, |v| v.label.as_str())
    }
}

fn sorted_index(variants: Option<&Vec<EnumVariant>>) -> Vec<u32> {
    let variants = variants.map_or(&[][..], Vec::as_slice);
    let name = |i: &u32| {
        variants
            .get(*i as usize)
            .map_or("", |variant| variant.name.as_str())
    };
    let mut index = (0..u32::try_from(variants.len()).unwrap_or(u32::MAX)).collect::<Vec<_>>();
    index.sort_unstable_by(|a, b| name(a).cmp(name(b)));
    index
}

fn find<'a>(
    variants: Option<&'a Vec<EnumVariant>>,
    index: &[u32],
    name: &str,
) -> Option<&'a EnumVariant> {
    let variants = variants?;
    let pos = index
        .binary_search_by(|i| {
            variants
                .get(*i as usize)
                .map_or("", |variant| variant.name.as_str())
                .cmp(name)
        })
        .ok()?;
    index.get(pos).and_then(|i| variants.get(*i as usize))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Error,
    Span,
    Info,
}

impl EventKind {
    pub fn of(name: &str) -> Self {
        if ERROR_MARKERS.iter().any(|marker| name.contains(marker)) {
            EventKind::Error
        } else if name.ends_with("-start") || name.ends_with("-end") {
            EventKind::Span
        } else {
            EventKind::Info
        }
    }

    fn icon(self, theme: &Theme) -> Span<'static> {
        match self {
            EventKind::Error => Span::styled("✕", Style::new().fg(theme.err)),
            EventKind::Span => Span::styled("◷", Style::new().fg(theme.info)),
            EventKind::Info => Span::styled("•", theme.muted()),
        }
    }
}

pub struct Timeline {
    top: usize,
    follow: bool,
    page: usize,
}

impl Timeline {
    pub fn position(&self) -> usize {
        self.top
    }

    pub fn new(follow: bool) -> Self {
        Timeline {
            top: 0,
            follow,
            page: 1,
        }
    }

    pub fn following(&self) -> bool {
        self.follow
    }

    pub fn scroll(&mut self, delta: isize, len: usize) {
        let top = if delta < 0 {
            self.top.saturating_sub(delta.unsigned_abs())
        } else {
            self.top.saturating_add(delta.unsigned_abs())
        };
        let last = len.saturating_sub(1);
        self.follow = delta > 0 && top >= last;
        self.top = top.min(last);
    }

    pub fn page(&mut self, forward: bool, len: usize) {
        let page = isize::try_from(self.page.max(1)).unwrap_or(isize::MAX);
        self.scroll(if forward { page } else { -page }, len);
    }

    pub fn home(&mut self) {
        self.top = 0;
        self.follow = false;
    }

    pub fn end(&mut self) {
        self.follow = true;
    }

    pub fn dropped(&mut self, count: usize) {
        self.top = self.top.saturating_sub(count);
    }

    pub fn reset(&mut self, follow: bool) {
        *self = Timeline::new(follow);
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        labels: &Labels,
        events: &[TraceEvent],
        anchor: Option<UtcTimestamp>,
    ) {
        if events.is_empty() {
            frame.render_widget(Paragraph::new(EMPTY).style(theme.muted()), area);
            return;
        }
        let height = usize::from(area.height);
        let mut lines = Vec::with_capacity(height + 8);
        let painter = Painter {
            theme,
            labels,
            anchor,
        };
        if self.follow {
            let mut rows = 0;
            let mut top = events.len();
            for event in events.iter().rev() {
                rows += event_rows(event);
                if rows > height && top < events.len() {
                    break;
                }
                top -= 1;
                if rows >= height {
                    break;
                }
            }
            self.top = top;
        }
        self.top = self.top.min(events.len().saturating_sub(1));
        let mut shown = 0;
        for event in events.iter().skip(self.top) {
            if lines.len() >= height {
                break;
            }
            painter.event(event, &mut lines);
            shown += 1;
        }
        self.page = shown.max(1);
        frame.render_widget(Paragraph::new(lines), area);
    }
}

pub struct Painter<'a> {
    pub theme: &'a Theme,
    pub labels: &'a Labels,
    pub anchor: Option<UtcTimestamp>,
}

fn event_rows(event: &TraceEvent) -> usize {
    1 + event
        .key_values
        .iter()
        .map(|kv| key_value_rows(kv, 1))
        .sum::<usize>()
}

fn key_value_rows(kv: &KeyValue, depth: usize) -> usize {
    match &kv.value {
        TraceValue::Event { value, .. } if depth <= NESTED_DEPTH => {
            1 + value
                .iter()
                .map(|child| key_value_rows(child, depth + 1))
                .sum::<usize>()
        }
        TraceValue::List { value } if value.len() > INLINE_LIST_MAX => value.len(),
        _ => 1,
    }
}

impl<'a> Painter<'a> {
    pub fn event(&self, event: &'a TraceEvent, out: &mut Vec<Line<'a>>) {
        let mut time = String::with_capacity(32);
        write_time(&mut time, event.timestamp);
        if let Some(anchor) = self.anchor {
            time.push_str(" +");
            write_elapsed(
                &mut time,
                event.timestamp.millis().saturating_sub(anchor.millis()),
            );
        }
        let mut spans = vec![
            EventKind::of(&event.event).icon(self.theme),
            Span::raw(" "),
            Span::styled(time, self.theme.muted()),
            Span::raw("  "),
        ];
        match self.labels.event(&event.event) {
            Some(variant) => {
                spans.push(Span::styled(variant.label.as_str(), self.theme.strong()));
                if let Some(explanation) = &variant.explanation {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(explanation.as_str(), self.theme.muted()));
                }
            }
            None => spans.push(Span::styled(
                &*event.event,
                self.theme.text().add_modifier(Modifier::BOLD),
            )),
        }
        out.push(Line::from(spans));
        for kv in &event.key_values {
            self.key_value(kv, 1, out);
        }
    }

    fn key_spans(&self, indent: usize, label: &'a str) -> [Span<'a>; 4] {
        let style = self.theme.muted();
        [
            Span::styled(pad(indent), style),
            Span::styled(pad(KEY_WIDTH.saturating_sub(label.chars().count())), style),
            Span::styled(label, style),
            Span::styled(KEY_GAP, style),
        ]
    }

    fn key_value(&self, kv: &'a KeyValue, depth: usize, out: &mut Vec<Line<'a>>) {
        let label = self.labels.key_label(&kv.key);
        let indent = depth * INDENT_WIDTH;
        match &kv.value {
            TraceValue::Event { event, value } if depth <= NESTED_DEPTH => {
                let mut spans = Vec::with_capacity(5);
                spans.extend(self.key_spans(indent, label));
                spans.push(Span::styled(
                    self.labels.event_label(event),
                    self.theme.strong(),
                ));
                out.push(Line::from(spans));
                for child in value {
                    self.key_value(child, depth + 1, out);
                }
            }
            TraceValue::List { value } if value.len() > INLINE_LIST_MAX => {
                let blank = indent + KEY_WIDTH.max(label.chars().count()) + KEY_GAP.len();
                for (index, item) in value.iter().enumerate() {
                    let mut text = String::new();
                    self.value(&mut text, &kv.key, item, 0);
                    let mut spans = Vec::with_capacity(5);
                    if index == 0 {
                        spans.extend(self.key_spans(indent, label));
                    } else {
                        let mut left = blank;
                        while left > 0 {
                            let chunk = pad(left);
                            left -= chunk.len();
                            spans.push(Span::styled(chunk, self.theme.muted()));
                        }
                    }
                    spans.push(Span::styled(text, self.theme.text()));
                    out.push(Line::from(spans));
                }
            }
            value => {
                let mut text = String::new();
                self.value(&mut text, &kv.key, value, 0);
                let style = if matches!(value, TraceValue::Null) {
                    self.theme.muted()
                } else {
                    self.theme.text()
                };
                let mut spans = Vec::with_capacity(5);
                spans.extend(self.key_spans(indent, label));
                spans.push(Span::styled(text, style));
                out.push(Line::from(spans));
            }
        }
    }

    fn value(&self, out: &mut String, key: &str, value: &TraceValue, depth: usize) {
        match value {
            TraceValue::String { value } | TraceValue::IpAddr { value } => out.push_str(value),
            TraceValue::UnsignedInt { value } => write_unsigned(out, key, *value),
            TraceValue::Integer { value } => match u64::try_from(*value) {
                Ok(value) => write_unsigned(out, key, value),
                Err(_) => {
                    out.push('-');
                    write_grouped(out, value.unsigned_abs());
                }
            },
            TraceValue::Boolean { value } => out.push_str(match (key == "tls", *value) {
                (true, true) => "enabled",
                (true, false) => "disabled",
                (false, true) => "true",
                (false, false) => "false",
            }),
            TraceValue::Float { value: Some(value) } => {
                let _ = write!(out, "{value:.2}");
            }
            TraceValue::Float { value: None } | TraceValue::Null => out.push_str("null"),
            TraceValue::UtcDateTime { value } => {
                let _ = value.format_iso(out);
            }
            TraceValue::Duration { value } => {
                write_elapsed(out, i64::try_from(*value).unwrap_or(i64::MAX))
            }
            TraceValue::List { value } => {
                for (index, item) in value.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    self.value(out, key, item, depth);
                }
            }
            TraceValue::Event { event, value } => {
                out.push_str(self.labels.event_label(event));
                if depth >= INLINE_DEPTH {
                    out.push_str(" {…}");
                    return;
                }
                out.push_str(" {");
                for (index, kv) in value.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(self.labels.key_label(&kv.key));
                    out.push_str(": ");
                    self.value(out, &kv.key, &kv.value, depth + 1);
                }
                out.push('}');
            }
        }
    }
}

fn write_unsigned(out: &mut String, key: &str, value: u64) {
    if key == "id" || key.ends_with("Id") {
        write_id(out, value);
    } else if key == "port" || key.ends_with("Port") {
        let _ = write!(out, "{value}");
    } else {
        write_grouped(out, value);
    }
}

pub fn write_id(out: &mut String, mut value: u64) {
    let mut digits = [0u8; 13];
    let mut len = 0;
    for digit in &mut digits {
        *digit = ID_ALPHABET[(value % 32) as usize];
        len += 1;
        value /= 32;
        if value == 0 {
            break;
        }
    }
    out.extend(digits.iter().take(len).rev().map(|b| char::from(*b)));
}

pub fn write_grouped(out: &mut String, mut value: u64) {
    let mut digits = [0u8; 20];
    let mut len = 0;
    for digit in &mut digits {
        *digit = b'0' + (value % 10) as u8;
        len += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for (index, digit) in digits.iter().take(len).rev().enumerate() {
        if index > 0 && (len - index) % 3 == 0 {
            out.push(',');
        }
        out.push(char::from(*digit));
    }
}

pub fn write_time(out: &mut String, at: UtcTimestamp) {
    let civil = at.civil();
    let _ = write!(
        out,
        "{:02}:{:02}:{:02}",
        civil.hour, civil.minute, civil.second
    );
    if civil.millis != 0 {
        let _ = write!(out, ".{:03}", civil.millis);
    }
}

pub fn write_elapsed(out: &mut String, ms: i64) {
    let _ = if ms <= 0 {
        out.write_str("0 ms")
    } else if ms < 1000 {
        write!(out, "{ms} ms")
    } else if (ms + 50) / 100 < 600 {
        let tenths = (ms + 50) / 100;
        write!(out, "{}.{} s", tenths / 10, tenths % 10)
    } else {
        let secs = (ms + 500) / 1000;
        let (mins, sec) = (secs / 60, secs % 60);
        match (mins < 60, sec) {
            (true, 0) => write!(out, "{mins} m"),
            (true, sec) => write!(out, "{mins} m {sec} s"),
            (false, _) if mins % 60 == 0 => write!(out, "{} h", mins / 60),
            (false, _) => write!(out, "{} h {} m", mins / 60, mins % 60),
        }
    };
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/widgets/timeline/tests.rs"]
mod tests;
