/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::theme::Theme;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

const MASK: char = '•';
const CURSOR: &str = "█";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Echo {
    Plain,
    Masked,
}

#[derive(Clone, Debug)]
pub struct Input {
    value: String,
    cursor: usize,
    echo: Echo,
    shown: String,
}

impl Default for Input {
    fn default() -> Self {
        Input::new("")
    }
}

impl Input {
    pub fn new(value: &str) -> Self {
        Input {
            value: value.to_string(),
            cursor: value.len(),
            echo: Echo::Plain,
            shown: String::new(),
        }
    }

    pub fn masked(value: &str) -> Self {
        Input {
            echo: Echo::Masked,
            ..Input::new(value)
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn set(&mut self, value: &str) {
        self.value.clear();
        self.value.push_str(value);
        self.cursor = self.value.len();
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub fn paste(&mut self, text: &str) {
        let mut inserted = 0;
        for c in text.chars().filter(|c| !c.is_control()) {
            self.value.insert(self.cursor + inserted, c);
            inserted += c.len_utf8();
        }
        self.cursor += inserted;
    }

    pub fn handle(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return self.handle_chord(key.code);
        }
        match key.code {
            KeyCode::Char(c) => {
                self.value.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace => {
                if let Some(c) = self.before().chars().next_back() {
                    self.cursor -= c.len_utf8();
                    self.value.remove(self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.value.len() {
                    self.value.remove(self.cursor);
                }
            }
            KeyCode::Left => {
                if let Some(c) = self.before().chars().next_back() {
                    self.cursor -= c.len_utf8();
                }
            }
            KeyCode::Right => {
                if let Some(c) = self.after().chars().next() {
                    self.cursor += c.len_utf8();
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.len(),
            _ => return false,
        }
        true
    }

    fn handle_chord(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('u') => {
                self.value.replace_range(..self.cursor, "");
                self.cursor = 0;
            }
            KeyCode::Char('k') => self.value.truncate(self.cursor),
            KeyCode::Char('a') => self.cursor = 0,
            KeyCode::Char('e') => self.cursor = self.value.len(),
            KeyCode::Char('w') | KeyCode::Backspace => {
                let before = self.before().trim_end();
                let start = before
                    .rsplit_once(char::is_whitespace)
                    .map_or(0, |(_, word)| before.len() - word.len());
                self.value.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            _ => return false,
        }
        true
    }

    fn before(&self) -> &str {
        self.value.get(..self.cursor).unwrap_or_default()
    }

    fn after(&self) -> &str {
        self.value.get(self.cursor..).unwrap_or_default()
    }

    pub fn line(&mut self, theme: &Theme, width: u16, active: bool) -> Line<'_> {
        let style = theme.input();
        let width = usize::from(width);
        self.shown.clear();
        let (before, after) = match self.echo {
            Echo::Plain => (self.before(), self.after()),
            Echo::Masked => {
                let n = self.before().chars().count();
                let m = self.after().chars().count();
                self.shown.extend(std::iter::repeat_n(MASK, n + m));
                let split = n * MASK.len_utf8();
                let (b, a) = self.shown.split_at(split);
                (b, a)
            }
        };
        let used = before.width() + after.width() + usize::from(active);
        let skip = used.saturating_sub(width);
        let before = skip_columns(before, skip);
        let pad = width.saturating_sub(used.min(width));
        let mut spans = Vec::with_capacity(4);
        spans.push(Span::styled(before, style));
        if active {
            spans.push(Span::styled(CURSOR, style.fg(theme.brand)));
        }
        spans.push(Span::styled(after, style));
        spans.push(Span::styled(" ".repeat(pad), style));
        Line::from(spans)
    }
}

fn skip_columns(text: &str, columns: usize) -> &str {
    if columns == 0 {
        return text;
    }
    let mut skipped = 0;
    for (pos, c) in text.char_indices() {
        if skipped >= columns {
            return text.get(pos..).unwrap_or_default();
        }
        skipped += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
    }
    ""
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/widgets/input/tests.rs"]
mod tests;
