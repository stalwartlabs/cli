/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::keymap::{code, ctrl, keys, resolve};
use crate::tui::theme::Theme;
use crate::tui::widgets::chrome::{centered, clear_popup};
use ahash::{AHashMap, AHashSet};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::fmt::Write;
use std::mem;

const WIDTH: u16 = 64;
const MAX_HEIGHT: u16 = 20;
const CHROME_ROWS: u16 = 4;
const PAGE: usize = 10;
const LABEL_COLUMN: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerAction {
    Up,
    Down,
    PageUp,
    PageDown,
    Choose,
    Toggle,
    Cancel,
}

keys! { pub KEYS, pub HINTS: PickerAction;
    "↑↓" Up "select" true [code(KeyCode::Up), ctrl('p')],
    "↓" Down "next" false [code(KeyCode::Down), ctrl('n')],
    "pgup" PageUp "page up" false [code(KeyCode::PageUp), ctrl('b')],
    "pgdn" PageDown "page down" false [code(KeyCode::PageDown), ctrl('f')],
    "⏎" Choose "choose" true [code(KeyCode::Enter)],
    "tab" Toggle "mark (multi-select)" false [code(KeyCode::Tab)],
    "esc" Cancel "cancel" true [code(KeyCode::Esc)],
}

#[derive(Debug, Clone)]
pub struct PickerItem {
    pub key: Box<str>,
    pub label: Box<str>,
    pub detail: Box<str>,
    pub color: Option<Color>,
    label_lower: Box<str>,
    detail_lower: Box<str>,
}

impl PickerItem {
    pub fn new(
        key: impl Into<Box<str>>,
        label: impl Into<Box<str>>,
        detail: impl Into<Box<str>>,
    ) -> Self {
        Self::build(key.into(), label.into(), detail.into())
    }

    fn build(key: Box<str>, label: Box<str>, detail: Box<str>) -> Self {
        Self {
            label_lower: label.to_lowercase().into_boxed_str(),
            detail_lower: detail.to_lowercase().into_boxed_str(),
            key,
            label,
            detail,
            color: None,
        }
    }

    pub fn with_color(mut self, color: Option<Color>) -> Self {
        self.color = color;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Single,
    Multi,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    QueryChanged,
    Cancel,
    Chosen(usize),
    ChosenMany(Vec<(Box<str>, Box<str>)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filtering {
    Local,
    Remote,
}

#[derive(Debug)]
enum Load {
    Ready,
    Waiting(Option<Box<str>>),
}

#[derive(Debug)]
pub struct Picker {
    title: Box<str>,
    load: Load,
    filtering: Filtering,
    items: Vec<PickerItem>,
    selection: Selection,
    current: Option<usize>,
    marked: AHashMap<Box<str>, Box<str>>,
    query: String,
    query_lower: String,
    matches: Vec<(u8, u32)>,
    selected: usize,
    offset: usize,
    status: Option<Box<str>>,
    scratch: String,
}

impl Picker {
    pub fn new(title: impl Into<Box<str>>, items: Vec<PickerItem>, selection: Selection) -> Self {
        let mut picker = Self {
            title: title.into(),
            load: Load::Ready,
            filtering: Filtering::Local,
            marked: AHashMap::new(),
            items,
            selection,
            current: None,
            query: String::new(),
            query_lower: String::new(),
            matches: Vec::new(),
            selected: 0,
            offset: 0,
            status: None,
            scratch: String::new(),
        };
        picker.refilter();
        picker
    }

    pub fn loading(title: impl Into<Box<str>>, selection: Selection) -> Self {
        let mut picker = Self::new(title, Vec::new(), selection);
        picker.status = Some("loading…".into());
        picker.load = Load::Waiting(None);
        picker
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.load, Load::Waiting(_))
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_filtering(&mut self, filtering: Filtering) {
        self.filtering = filtering;
    }

    pub fn reload(&mut self, status: impl Into<Box<str>>) {
        let current = self
            .current
            .and_then(|index| self.items.get(index))
            .map(|item| item.key.clone());
        self.load = Load::Waiting(current);
        self.status = Some(status.into());
    }

    pub fn set_items(&mut self, items: Vec<PickerItem>) {
        let current = match mem::replace(&mut self.load, Load::Ready) {
            Load::Waiting(current) => current,
            Load::Ready => self
                .current
                .and_then(|index| self.items.get(index))
                .map(|item| item.key.clone()),
        };
        self.items = items;
        self.status = None;
        self.refilter();
        self.mark_current(current.as_deref());
    }

    pub fn set_status(&mut self, status: impl Into<Box<str>>) {
        self.status = Some(status.into());
    }

    pub fn with_current(mut self, key: Option<&str>) -> Self {
        match &mut self.load {
            Load::Waiting(current) => *current = key.map(Box::from),
            Load::Ready => self.mark_current(key),
        }
        self
    }

    pub fn with_marked<'k>(mut self, marks: impl IntoIterator<Item = (&'k str, &'k str)>) -> Self {
        self.marked.extend(
            marks
                .into_iter()
                .map(|(key, label)| (Box::from(key), Box::from(label))),
        );
        self
    }

    fn mark_current(&mut self, key: Option<&str>) {
        self.current = key.and_then(|key| self.items.iter().position(|item| &*item.key == key));
        if let Some(current) = self.current
            && let Some(pos) = self
                .matches
                .iter()
                .position(|&(_, i)| i as usize == current)
        {
            self.selected = pos;
        }
    }

    pub fn item(&self, index: usize) -> Option<&PickerItem> {
        self.items.get(index)
    }

    fn chosen(&self) -> Vec<(Box<str>, Box<str>)> {
        let mut chosen: Vec<(Box<str>, Box<str>)> = self
            .items
            .iter()
            .filter(|item| self.marked.contains_key(&*item.key))
            .map(|item| (item.key.clone(), item.label.clone()))
            .collect();
        if chosen.len() < self.marked.len() {
            let mut unseen: Vec<(Box<str>, Box<str>)> = {
                let shown: AHashSet<&str> = chosen.iter().map(|(key, _)| &**key).collect();
                self.marked
                    .iter()
                    .filter(|(key, _)| !shown.contains(&***key))
                    .map(|(key, label)| (key.clone(), label.clone()))
                    .collect()
            };
            unseen.sort_unstable();
            chosen.append(&mut unseen);
        }
        chosen
    }

    pub fn highlighted(&self) -> Option<usize> {
        self.matches.get(self.selected).map(|&(_, i)| i as usize)
    }

    pub fn handle(&mut self, key: KeyEvent) -> Outcome {
        if let Some(action) = resolve(KEYS, key) {
            return self.act(action);
        }
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.push(c);
                self.refilter();
                Outcome::QueryChanged
            }
            KeyCode::Backspace => {
                if self.query.pop().is_none() {
                    return Outcome::Pending;
                }
                self.refilter();
                Outcome::QueryChanged
            }
            _ => Outcome::Pending,
        }
    }

    pub fn paste(&mut self, text: &str) -> Outcome {
        self.query.extend(text.chars().filter(|c| !c.is_control()));
        self.refilter();
        Outcome::QueryChanged
    }

    pub fn act(&mut self, action: PickerAction) -> Outcome {
        let last = self.matches.len().saturating_sub(1);
        match action {
            PickerAction::Up => self.selected = self.selected.saturating_sub(1),
            PickerAction::Down => self.selected = (self.selected + 1).min(last),
            PickerAction::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            PickerAction::PageDown => self.selected = (self.selected + PAGE).min(last),
            PickerAction::Cancel => return Outcome::Cancel,
            PickerAction::Toggle => {
                if self.selection == Selection::Multi
                    && let Some(item) = self.highlighted().and_then(|index| self.items.get(index))
                {
                    if self.marked.remove(&item.key).is_none() {
                        self.marked.insert(item.key.clone(), item.label.clone());
                    }
                    self.selected = (self.selected + 1).min(last);
                }
            }
            PickerAction::Choose => {
                return match self.selection {
                    Selection::Single => {
                        self.highlighted().map_or(Outcome::Pending, Outcome::Chosen)
                    }
                    Selection::Multi => {
                        if self.marked.is_empty()
                            && let Some(item) =
                                self.highlighted().and_then(|index| self.items.get(index))
                        {
                            self.marked.insert(item.key.clone(), item.label.clone());
                        }
                        Outcome::ChosenMany(self.chosen())
                    }
                };
            }
        }
        Outcome::Pending
    }

    fn refilter(&mut self) {
        self.query_lower.clear();
        self.query_lower
            .extend(self.query.chars().flat_map(char::to_lowercase));
        self.matches.clear();
        let needle = self.query_lower.as_str();
        self.matches
            .extend(self.items.iter().enumerate().filter_map(|(i, item)| {
                let rank = if needle.is_empty() || item.label_lower.starts_with(needle) {
                    0
                } else if item.label_lower.contains(needle) {
                    1
                } else if item.detail_lower.contains(needle) || item.key.contains(needle) {
                    2
                } else if self.filtering == Filtering::Remote {
                    3
                } else {
                    return None;
                };
                Some((rank, u32::try_from(i).ok()?))
            }));
        self.matches.sort_by_key(|&(rank, _)| rank);
        self.selected = 0;
        self.offset = 0;
    }

    pub fn render(&mut self, frame: &mut Frame, anchor: Rect, theme: &Theme) {
        let rows = u16::try_from(self.matches.len()).unwrap_or(u16::MAX);
        let height = rows.max(1).saturating_add(CHROME_ROWS).min(MAX_HEIGHT);
        let area = centered(anchor, WIDTH, height);
        clear_popup(frame, area, theme);
        self.scratch.clear();
        let _ = write!(
            self.scratch,
            "{} of {}",
            self.matches.len(),
            self.items.len()
        );
        if self.selection == Selection::Multi {
            let _ = write!(self.scratch, " · {} marked · tab mark", self.marked.len());
        }
        self.scratch.push_str(" · type to search ");
        let title = Line::from(vec![
            Span::raw(" "),
            Span::raw(&*self.title),
            Span::raw(" "),
        ]);
        let block = theme
            .popup_block(title, theme.accent)
            .title_bottom(Line::styled(self.scratch.as_str(), theme.muted()).right_aligned());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let visible = usize::from(inner.height.saturating_sub(2));
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if visible > 0 && self.selected >= self.offset + visible {
            self.offset = self.selected + 1 - visible;
        }

        let mut lines = Vec::with_capacity(visible + 2);
        lines.push(Line::from(vec![
            Span::styled(" ⌕ ", theme.muted()),
            Span::styled(self.query.as_str(), theme.text()),
            Span::styled("█", Style::new().fg(theme.brand)),
        ]));
        lines.push(Line::default());
        if let Some(status) = &self.status {
            lines.push(Line::styled(format!(" {status}"), theme.muted()));
        } else if self.matches.is_empty() {
            lines.push(Line::styled(" no match", theme.muted()));
        }
        let width = usize::from(inner.width);
        for (pos, &(_, index)) in self
            .matches
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(visible)
        {
            let Some(item) = self.items.get(index as usize) else {
                continue;
            };
            let index = index as usize;
            let focused = pos == self.selected;
            let marker = match self.selection {
                Selection::Single if self.current == Some(index) => "✓ ",
                Selection::Multi if self.marked.contains_key(&*item.key) => "◉ ",
                Selection::Multi => "○ ",
                Selection::Single => "  ",
            };
            let base = if focused {
                theme.selected
            } else {
                theme.text()
            };
            let detail = if focused {
                theme.selected
            } else {
                theme.muted()
            };
            let mut spans = vec![
                Span::styled(" ", base),
                Span::styled(
                    marker,
                    if focused {
                        base
                    } else {
                        Style::new().fg(theme.accent)
                    },
                ),
            ];
            if let Some(color) = item.color {
                spans.push(Span::styled("● ", Style::new().fg(color)));
            }
            spans.push(Span::styled(
                &*item.label,
                base.add_modifier(Modifier::BOLD),
            ));
            let label_width = unicode_width::UnicodeWidthStr::width(&*item.label);
            let pad = LABEL_COLUMN.saturating_sub(label_width).max(2);
            spans.push(Span::styled(" ".repeat(pad), base));
            spans.push(Span::styled(&*item.detail, detail));
            let mut line = Line::from(spans);
            if focused {
                let used = line.width();
                line.spans
                    .push(Span::styled(" ".repeat(width.saturating_sub(used)), base));
            }
            lines.push(line);
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/widgets/picker/tests.rs"]
mod tests;

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/widgets/picker/test_support.rs"]
mod test_support;
