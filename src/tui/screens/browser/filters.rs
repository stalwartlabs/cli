/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::input::filter::CompareOp;
use crate::input::filter::list::{AppliedFilter, FilterValue, build_list_filter};
use crate::schema::view::ViewDef;
use crate::schema::{Filter, Schema};
use crate::tui::keymap::{code, ctrl, keys, shifted};
use crate::tui::theme::Theme;
use crate::tui::widgets::input::Input;
use crate::tui::widgets::picker::PickerItem;
use crate::view::Color;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use serde_json::Value;

const OPS: [CompareOp; 5] = [
    CompareOp::Eq,
    CompareOp::Gte,
    CompareOp::Gt,
    CompareOp::Lte,
    CompareOp::Lt,
];
const INPUT_WIDTH: u16 = 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterAction {
    Next,
    Prev,
    Apply,
    Operator,
    ClearField,
    Close,
}

keys! { pub KEYS, pub HINTS: FilterAction;
    "tab" Next "next filter" true [code(KeyCode::Tab)],
    "⇧tab" Prev "previous filter" false [shifted(KeyCode::BackTab), code(KeyCode::BackTab)],
    "⏎" Apply "apply / choose" true [code(KeyCode::Enter)],
    "^o" Operator "operator" true [ctrl('o')],
    "^x" ClearField "clear field" true [ctrl('x')],
    "esc" Close "close" true [code(KeyCode::Esc)],
}

#[derive(Debug)]
enum EntryValue {
    Text(Input),
    Choice(Vec<(Box<str>, Box<str>)>),
}

#[derive(Debug)]
struct Entry {
    index: usize,
    op: CompareOp,
    value: EntryValue,
}

pub enum FilterEvent {
    Pending,
    Apply,
    Close,
    Choose(usize),
}

#[derive(Debug, Default)]
pub struct Filters {
    entries: Vec<Entry>,
    focus: usize,
    editing: bool,
}

impl Filters {
    pub fn new(view: Option<&ViewDef<'_>>, visible: impl Fn(&Filter) -> bool) -> Self {
        let filters = view.and_then(|v| v.list).map_or(&[][..], |l| &l.filters);
        let entries = filters
            .iter()
            .enumerate()
            .filter(|(_, f)| visible(f))
            .map(|(index, filter)| Entry {
                index,
                op: CompareOp::Eq,
                value: match filter {
                    Filter::Enum { .. } | Filter::ObjectId { .. } => EntryValue::Choice(Vec::new()),
                    Filter::Text { .. } | Filter::Integer { .. } | Filter::Date { .. } => {
                        EntryValue::Text(Input::default())
                    }
                },
            })
            .collect();
        Self {
            entries,
            focus: 0,
            editing: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn is_editing(&self) -> bool {
        self.editing
    }

    pub fn open(&mut self) {
        self.editing = !self.entries.is_empty();
    }

    pub fn is_active(&self) -> bool {
        self.entries.iter().any(|entry| match &entry.value {
            EntryValue::Text(input) => !input.value().trim().is_empty(),
            EntryValue::Choice(items) => !items.is_empty(),
        })
    }

    pub fn act(&mut self, action: FilterAction, list: &[Filter]) -> FilterEvent {
        let count = self.entries.len().max(1);
        match action {
            FilterAction::Next => self.focus = (self.focus + 1) % count,
            FilterAction::Prev => self.focus = (self.focus + count - 1) % count,
            FilterAction::Close => {
                self.editing = false;
                return FilterEvent::Close;
            }
            FilterAction::ClearField => {
                if let Some(entry) = self.entries.get_mut(self.focus) {
                    match &mut entry.value {
                        EntryValue::Text(input) => input.clear(),
                        EntryValue::Choice(items) => items.clear(),
                    }
                    entry.op = CompareOp::Eq;
                }
                return FilterEvent::Apply;
            }
            FilterAction::Operator => {
                if let Some(entry) = self.entries.get_mut(self.focus)
                    && matches!(
                        list.get(entry.index),
                        Some(Filter::Integer { .. } | Filter::Date { .. })
                    )
                {
                    let next = OPS
                        .iter()
                        .position(|op| *op == entry.op)
                        .map_or(0, |i| i + 1);
                    entry.op = OPS.get(next).copied().unwrap_or(CompareOp::Eq);
                }
            }
            FilterAction::Apply => {
                return match self.entries.get(self.focus) {
                    Some(Entry {
                        index,
                        value: EntryValue::Choice(_),
                        ..
                    }) => FilterEvent::Choose(*index),
                    _ => {
                        self.editing = false;
                        FilterEvent::Apply
                    }
                };
            }
        }
        FilterEvent::Pending
    }

    pub fn text(&mut self, key: KeyEvent) {
        if let Some(Entry {
            value: EntryValue::Text(input),
            ..
        }) = self.entries.get_mut(self.focus)
        {
            input.handle(key);
        }
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(Entry {
            value: EntryValue::Text(input),
            ..
        }) = self.entries.get_mut(self.focus)
        {
            input.paste(text);
        }
    }

    pub fn chosen(&mut self, index: usize, items: Vec<(Box<str>, Box<str>)>) {
        if let Some(Entry {
            value: EntryValue::Choice(current),
            ..
        }) = self.entries.iter_mut().find(|e| e.index == index)
        {
            *current = items;
        }
    }

    pub fn current(&self, index: usize) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .filter(move |e| e.index == index)
            .flat_map(|e| match &e.value {
                EntryValue::Choice(items) => items.as_slice(),
                EntryValue::Text(_) => &[],
            })
            .map(|(key, label)| (&**key, &**label))
    }

    pub fn build(&self, view: &ViewDef<'_>) -> Result<Option<Value>> {
        let Some(list) = view.list else {
            return Ok(None);
        };
        let mut keys: Vec<Box<[Box<str>]>> = Vec::new();
        for entry in &self.entries {
            if let EntryValue::Choice(items) = &entry.value {
                keys.push(items.iter().map(|(key, _)| key.clone()).collect());
            }
        }
        let mut keys = keys.iter();
        let applied: Vec<AppliedFilter<'_>> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let filter = list.filters.get(entry.index)?;
                let value = match &entry.value {
                    EntryValue::Text(input) => FilterValue::One(input.value()),
                    EntryValue::Choice(_) => FilterValue::Many(keys.next()?),
                };
                Some(AppliedFilter {
                    filter,
                    op: entry.op,
                    value,
                })
            })
            .collect();
        let filter = build_list_filter(view, &applied)?;
        Ok(filter
            .as_object()
            .is_some_and(|map| !map.is_empty())
            .then_some(filter))
    }

    pub fn line<'a>(&'a mut self, theme: &Theme, list: &'a [Filter]) -> Line<'a> {
        let mut spans = Vec::with_capacity(self.entries.len() * 4 + 1);
        spans.push(Span::styled(" / ", theme.muted()));
        let editing = self.editing;
        for (pos, entry) in self.entries.iter_mut().enumerate() {
            let Some(filter) = list.get(entry.index) else {
                continue;
            };
            let focused = editing && pos == self.focus;
            let label_style = if focused {
                theme.title().add_modifier(Modifier::UNDERLINED)
            } else {
                theme.muted()
            };
            spans.push(Span::styled(filter.label(), label_style));
            spans.push(Span::raw(" "));
            if entry.op != CompareOp::Eq {
                spans.push(Span::styled(entry.op.symbol(), theme.title()));
                spans.push(Span::raw(" "));
            }
            match &mut entry.value {
                EntryValue::Text(input) => {
                    if focused {
                        spans.extend(input.line(theme, INPUT_WIDTH, true).spans);
                    } else if input.is_empty() {
                        spans.push(Span::styled("any", theme.muted()));
                    } else {
                        spans.push(Span::styled(input.value(), theme.text()));
                    }
                }
                EntryValue::Choice(items) if items.is_empty() => {
                    spans.push(Span::styled("any ▾", theme.muted()));
                }
                EntryValue::Choice(items) => {
                    for (i, (_, label)) in items.iter().enumerate() {
                        if i > 0 {
                            spans.push(Span::styled(", ", theme.muted()));
                        }
                        spans.push(Span::styled(&**label, theme.text()));
                    }
                    spans.push(Span::styled(" ▾", theme.muted()));
                }
            }
            spans.push(Span::raw("   "));
        }
        Line::from(spans)
    }
}

pub fn enum_items(schema: &Schema, enum_name: &str) -> Vec<PickerItem> {
    schema
        .enums
        .get(enum_name)
        .map(|variants| {
            variants
                .iter()
                .map(|v| {
                    PickerItem::new(
                        v.name.as_str(),
                        if v.label.is_empty() {
                            v.name.as_str()
                        } else {
                            v.label.as_str()
                        },
                        v.explanation.as_deref().unwrap_or_default(),
                    )
                    .with_color(v.color.as_deref().and_then(swatch))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn swatch(raw: &str) -> Option<ratatui::style::Color> {
    match Color::parse(raw) {
        Color::Rgb(r, g, b) => Some(ratatui::style::Color::Rgb(r, g, b)),
        _ => None,
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/browser/filters/tests.rs"]
mod tests;
