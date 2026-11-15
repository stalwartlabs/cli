/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::keymap::{code, ctrl, keys, shifted};
use crate::tui::route::Route;
use crate::tui::theme::Theme;
use crate::tui::widgets::chrome::{clear_popup, pad};
use crate::tui::widgets::input::Input;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::cmp::Reverse;
use std::fmt::Write;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

mod index;

pub const MAX_HITS: usize = 15;
const ROWS: u16 = 12;
const WIDTH: u16 = 92;
const TOP: u16 = 3;
const LABEL_COLUMN: usize = 28;
const PATH_COLUMN: usize = 38;
const ELLIPSIS: &str = "…";
const KEYWORD_PENALTY: i32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    Down,
    Up,
    Confirm,
    Close,
}

keys! { pub KEYS, pub HINTS: PaletteAction;
    "↑↓" Down "select" true [code(KeyCode::Down), ctrl('n'), code(KeyCode::Tab)],
    "↑" Up "up" false [code(KeyCode::Up), ctrl('p'), shifted(KeyCode::BackTab)],
    "⏎" Confirm "open" true [code(KeyCode::Enter)],
    "esc" Close "close" true [code(KeyCode::Esc)],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    Link,
    Form,
    Field,
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Tools,
    Accounts,
    TryEnterprise,
    HttpLog,
    Logout,
    Theme,
    Sidebar,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteTarget {
    Route(Route),
    Command(Command),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LabelSearch {
    Fuzzy,
    Substring,
}

pub struct Entry {
    pub kind: EntryKind,
    pub label: Box<str>,
    pub path: Box<str>,
    pub detail: Box<str>,
    pub target: PaletteTarget,
    label_hay: Box<str>,
    keyword_hay: Box<str>,
    search: LabelSearch,
}

impl Entry {
    pub fn new(
        kind: EntryKind,
        label: &str,
        path: &str,
        detail: &str,
        keywords: &str,
        target: PaletteTarget,
    ) -> Self {
        let mut label_hay = String::with_capacity(label.len());
        fold_into(&mut label_hay, label);
        let mut keyword_hay = String::with_capacity(keywords.len() + detail.len() + 1);
        fold_into(&mut keyword_hay, keywords);
        keyword_hay.push(' ');
        fold_into(&mut keyword_hay, detail);
        Entry {
            kind,
            label: label.into(),
            path: path.into(),
            detail: detail.into(),
            target,
            label_hay: label_hay.into(),
            keyword_hay: keyword_hay.into(),
            search: LabelSearch::Fuzzy,
        }
    }

    pub fn text_label(mut self) -> Self {
        self.search = LabelSearch::Substring;
        self
    }
}

fn fold_into(out: &mut String, text: &str) {
    out.extend(text.chars().map(|c| c.to_lowercase().next().unwrap_or(c)));
}

#[derive(Default)]
pub struct PaletteIndex {
    entries: Vec<Entry>,
}

impl PaletteIndex {
    pub fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, index: u32) -> Option<&Entry> {
        self.entries.get(usize::try_from(index).ok()?)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum LabelMatch {
    Prefix,
    Contains,
    Fuzzy,
    Description,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hit {
    entry: u32,
    label: LabelMatch,
    kind: EntryKind,
    score: i32,
}

pub enum Outcome {
    Stay,
    Close,
    Open(PaletteTarget),
}

const NO_MATCHES: &str = "   No matches. Try part of a page title, such as queue, dkim or logs.";

#[derive(Default)]
pub struct PaletteState {
    pub input: Input,
    selected: usize,
    hits: Vec<Hit>,
    query: String,
    positions: Vec<usize>,
    status: String,
}

impl PaletteState {
    pub fn new(index: &PaletteIndex) -> Self {
        let mut state = PaletteState::default();
        state.refresh(index);
        state
    }

    pub fn refresh(&mut self, index: &PaletteIndex) {
        self.query.clear();
        fold_into(&mut self.query, self.input.value());
        self.hits.clear();
        let prefix = self.query.trim();
        for (i, entry) in index.entries.iter().enumerate() {
            if let Some(score) = score_entry(&self.query, entry, None)
                && let Ok(id) = u32::try_from(i)
            {
                let label = match entry.search {
                    LabelSearch::Substring => LabelMatch::Description,
                    LabelSearch::Fuzzy if entry.label_hay.starts_with(prefix) => LabelMatch::Prefix,
                    LabelSearch::Fuzzy if entry.label_hay.contains(prefix) => LabelMatch::Contains,
                    LabelSearch::Fuzzy => LabelMatch::Fuzzy,
                };
                self.hits.push(Hit {
                    entry: id,
                    label,
                    kind: entry.kind,
                    score,
                });
            }
        }
        self.hits
            .sort_unstable_by_key(|hit| (hit.label, hit.kind, Reverse(hit.score), hit.entry));
        self.hits.truncate(MAX_HITS);
        self.selected = 0;
    }

    pub fn handle(
        &mut self,
        index: &PaletteIndex,
        action: Option<PaletteAction>,
        key: KeyEvent,
    ) -> Outcome {
        match action {
            Some(PaletteAction::Close) => Outcome::Close,
            Some(PaletteAction::Down) => {
                self.selected = (self.selected + 1).min(self.hits.len().saturating_sub(1));
                Outcome::Stay
            }
            Some(PaletteAction::Up) => {
                self.selected = self.selected.saturating_sub(1);
                Outcome::Stay
            }
            Some(PaletteAction::Confirm) => self
                .hits
                .get(self.selected)
                .and_then(|hit| index.get(hit.entry))
                .map_or(Outcome::Stay, |entry| Outcome::Open(entry.target.clone())),
            None => {
                if self.input.handle(key) {
                    self.refresh(index);
                }
                Outcome::Stay
            }
        }
    }

    pub fn paste(&mut self, index: &PaletteIndex, text: &str) {
        self.input.paste(text);
        self.refresh(index);
    }

    pub fn render(&mut self, frame: &mut Frame, theme: &Theme, index: &PaletteIndex) {
        let full = frame.area();
        let width = WIDTH.min(full.width);
        let height = (ROWS + 5).min(full.height.saturating_sub(TOP.min(full.height)));
        let area = Rect::new(
            full.x + (full.width - width) / 2,
            full.y + TOP.min(full.height),
            width,
            height,
        );
        clear_popup(frame, area, theme);
        self.status.clear();
        let _ = write!(
            self.status,
            " {} of {} · ↑↓ select · ⏎ open · esc close ",
            self.hits.len(),
            index.len()
        );
        let block = theme
            .popup_block(" Jump to ", theme.brand)
            .title_bottom(Line::styled(self.status.as_str(), theme.muted()).right_aligned());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let [input_area, _, list] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(inner);
        let mut line = Line::from(Span::styled(" : ", theme.strong()));
        line.spans.extend(
            self.input
                .line(theme, inner.width.saturating_sub(4), true)
                .spans,
        );
        frame.render_widget(Paragraph::new(line), input_area);

        let rows = usize::from(list.height);
        let start = self.selected.saturating_sub(rows.saturating_sub(1));
        let hit_style = Style::new()
            .fg(theme.brand)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        let mut lines = Vec::with_capacity(rows.min(self.hits.len()));
        for (n, hit) in self.hits.iter().enumerate().skip(start).take(rows) {
            let Some(entry) = index.get(hit.entry) else {
                continue;
            };
            let selected = n == self.selected;
            let base = if selected {
                theme.selected
            } else {
                theme.text()
            };
            let hit = if selected {
                theme.selected.add_modifier(Modifier::UNDERLINED)
            } else {
                hit_style
            };
            let muted = if selected {
                theme.selected
            } else {
                theme.muted()
            };
            let accent = if selected {
                theme.selected
            } else {
                Style::new().fg(theme.accent)
            };
            self.positions.clear();
            score_entry(&self.query, entry, Some(&mut self.positions));
            self.positions.sort_unstable();
            let mut spans = Vec::with_capacity(8);
            spans.push(Span::styled(if selected { " ▶ " } else { "   " }, base));
            let (label, label_width) = fit(&entry.label, LABEL_COLUMN);
            highlight(&mut spans, label, &self.positions, base, hit);
            if label.len() < entry.label.len() {
                spans.push(Span::styled(ELLIPSIS, base));
            }
            spans.push(Span::styled(
                pad(LABEL_COLUMN.saturating_sub(label_width)),
                base,
            ));
            let (path, path_width) = fit(&entry.path, PATH_COLUMN);
            spans.push(Span::styled(path, muted));
            if path.len() < entry.path.len() {
                spans.push(Span::styled(ELLIPSIS, muted));
            }
            spans.push(Span::styled(
                pad(PATH_COLUMN.saturating_sub(path_width)),
                muted,
            ));
            spans.push(Span::styled(entry.detail.as_ref(), accent));
            lines.push(Line::from(spans));
        }
        if lines.is_empty() && !self.input.value().is_empty() {
            lines.push(Line::styled(NO_MATCHES, theme.muted()));
        }
        frame.render_widget(Paragraph::new(lines), list);
    }
}

fn fit(text: &str, column: usize) -> (&str, usize) {
    let width = text.width();
    if width < column {
        return (text, width);
    }
    let budget = column.saturating_sub(2);
    let mut used = 0;
    for (at, c) in text.char_indices() {
        let next = used + c.width().unwrap_or(0);
        if next > budget {
            return (text.get(..at).unwrap_or_default(), used + 1);
        }
        used = next;
    }
    (text, width)
}

fn score_entry(query: &str, entry: &Entry, mut positions: Option<&mut Vec<usize>>) -> Option<i32> {
    let mut total = 0;
    for token in query.split_whitespace() {
        let on_label = match entry.search {
            LabelSearch::Fuzzy => fuzzy(token, &entry.label_hay, positions.as_deref_mut()),
            LabelSearch::Substring => substring(token, &entry.label_hay, positions.as_deref_mut()),
        };
        total += match on_label {
            Some(score) => score,
            None => substring(token, &entry.keyword_hay, None)? - KEYWORD_PENALTY,
        };
    }
    Some(total)
}

fn fuzzy(needle: &str, hay: &str, mut positions: Option<&mut Vec<usize>>) -> Option<i32> {
    let mut wanted = needle.chars().peekable();
    let mark = positions.as_ref().map_or(0, |p| p.len());
    let mut score = 0;
    let mut first = None;
    let mut prev_hit: Option<usize> = None;
    let mut prev_char = ' ';
    for (i, c) in hay.chars().enumerate() {
        let Some(&w) = wanted.peek() else { break };
        if c == w {
            score += 10;
            if prev_hit.is_some_and(|p| p + 1 == i) {
                score += 15;
            }
            if i == 0 || word_start(prev_char) {
                score += 20;
            }
            first.get_or_insert(i);
            if let Some(p) = positions.as_deref_mut() {
                p.push(i);
            }
            prev_hit = Some(i);
            wanted.next();
        }
        prev_char = c;
    }
    if wanted.peek().is_some() {
        if let Some(p) = positions {
            p.truncate(mark);
        }
        return None;
    }
    Some(
        score
            - i32::try_from(first.unwrap_or(0))
                .unwrap_or(i32::MAX)
                .min(30),
    )
}

fn substring(needle: &str, hay: &str, positions: Option<&mut Vec<usize>>) -> Option<i32> {
    let (before, _) = hay.split_once(needle)?;
    let first = before.chars().count();
    let len = needle.chars().count();
    if let Some(p) = positions {
        p.extend(first..first + len);
    }
    let len = i32::try_from(len).unwrap_or(i32::MAX);
    let start = if before.chars().next_back().is_none_or(word_start) {
        20
    } else {
        0
    };
    let offset = i32::try_from(first).unwrap_or(i32::MAX).min(30);
    Some(len.saturating_mul(25).saturating_sub(15) + start - offset)
}

fn word_start(prev: char) -> bool {
    matches!(prev, ' ' | ':' | '/' | '-' | '_' | '.')
}

pub fn highlight<'a>(
    spans: &mut Vec<Span<'a>>,
    text: &'a str,
    positions: &[usize],
    base: Style,
    hit: Style,
) {
    let mut start = 0;
    let mut in_hit = false;
    for (ci, (bi, _)) in text.char_indices().enumerate() {
        let is_hit = positions.binary_search(&ci).is_ok();
        if is_hit != in_hit {
            if let Some(piece) = text.get(start..bi).filter(|p| !p.is_empty()) {
                spans.push(Span::styled(piece, if in_hit { hit } else { base }));
            }
            start = bi;
            in_hit = is_hit;
        }
    }
    if let Some(piece) = text.get(start..).filter(|p| !p.is_empty()) {
        spans.push(Span::styled(piece, if in_hit { hit } else { base }));
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/widgets/palette/tests.rs"]
mod tests;
