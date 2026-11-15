/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::theme::Theme;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

const MASK: &str = "•";
const CURSOR: &str = "█";
const MIN_WRAP_WIDTH: usize = 2;

#[derive(Debug, Clone)]
pub struct TextArea {
    lines: Vec<String>,
    row: usize,
    col: usize,
    scroll: usize,
    masked: bool,
}

impl TextArea {
    pub fn new(text: &str) -> Self {
        let mut lines: Vec<String> = text
            .split('\n')
            .map(|l| l.trim_end_matches('\r').to_string())
            .collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let row = lines.len() - 1;
        let col = lines.last().map_or(0, |l| l.chars().count());
        Self {
            lines,
            row,
            col,
            scroll: 0,
            masked: false,
        }
    }

    pub fn at_top(mut self) -> Self {
        self.row = 0;
        self.col = 0;
        self.scroll = 0;
        self
    }

    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    pub fn set_masked(&mut self, masked: bool) {
        self.masked = masked;
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    fn current(&mut self) -> &mut String {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let last = self.lines.len() - 1;
        let row = self.row.min(last);
        &mut self.lines[row]
    }

    fn byte_at(line: &str, col: usize) -> usize {
        line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines.get(row).map_or(0, |l| l.chars().count())
    }

    pub fn handle(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        match key.code {
            KeyCode::Char(c) => self.insert(c),
            KeyCode::Enter => self.newline(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => {
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.line_len(self.row);
                }
            }
            KeyCode::Right => {
                if self.col < self.line_len(self.row) {
                    self.col += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
            }
            KeyCode::Up => {
                self.row = self.row.saturating_sub(1);
                self.col = self.col.min(self.line_len(self.row));
            }
            KeyCode::Down => {
                self.row = (self.row + 1).min(self.lines.len().saturating_sub(1));
                self.col = self.col.min(self.line_len(self.row));
            }
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = self.line_len(self.row),
            KeyCode::Tab => {
                for _ in 0..4 {
                    self.insert(' ');
                }
            }
            _ => return false,
        }
        true
    }

    pub fn paste(&mut self, text: &str) {
        let col = self.col;
        let line = self.current();
        let at = Self::byte_at(line, col);
        let tail = line.split_off(at);
        let mut segments = text.split('\n');
        let first = segments.next().unwrap_or_default();
        line.extend(first.split('\r'));
        let row = self.row.min(self.lines.len().saturating_sub(1));
        let before = self.lines.len();
        self.lines.splice(
            row + 1..row + 1,
            segments.map(|segment| segment.split('\r').collect::<String>()),
        );
        let added = self.lines.len() - before;
        self.row = row + added;
        self.col = if added == 0 {
            col + first.chars().filter(|c| *c != '\r').count()
        } else {
            self.line_len(self.row)
        };
        self.current().push_str(&tail);
    }

    fn insert(&mut self, c: char) {
        let col = self.col;
        let line = self.current();
        let at = Self::byte_at(line, col);
        line.insert(at, c);
        self.col += 1;
    }

    fn newline(&mut self) {
        let col = self.col;
        let line = self.current();
        let at = Self::byte_at(line, col);
        let rest = line.split_off(at);
        self.row = self.row.min(self.lines.len() - 1) + 1;
        self.lines.insert(self.row, rest);
        self.col = 0;
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            let col = self.col - 1;
            let line = self.current();
            let at = Self::byte_at(line, col);
            line.remove(at);
            self.col = col;
        } else if self.row > 0 && self.row < self.lines.len() {
            let removed = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.line_len(self.row);
            self.current().push_str(&removed);
        }
    }

    fn delete(&mut self) {
        let col = self.col;
        if col < self.line_len(self.row) {
            let line = self.current();
            let at = Self::byte_at(line, col);
            line.remove(at);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.current().push_str(&next);
        }
    }

    pub fn lines<'a>(
        &'a mut self,
        theme: &Theme,
        height: usize,
        active: bool,
    ) -> impl Iterator<Item = Line<'a>> {
        if self.row < self.scroll {
            self.scroll = self.row;
        } else if height > 0 && self.row >= self.scroll + height {
            self.scroll = self.row + 1 - height;
        }
        let (row, col, masked) = (self.row, self.col, self.masked);
        let input = theme.input();
        let cursor = input.fg(theme.brand);
        self.lines
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(height.max(1))
            .map(move |(i, text)| {
                if masked {
                    return Line::styled(MASK.repeat(text.chars().count().max(1)), input);
                }
                if !active || i != row {
                    return Line::styled(text.as_str(), input);
                }
                cursor_line(text, Self::byte_at(text, col), input, cursor)
            })
    }

    pub fn wrapped<'a>(&'a mut self, theme: &Theme, width: usize, height: usize) -> Vec<Line<'a>> {
        let width = width.max(MIN_WRAP_WIDTH);
        let height = height.max(1);
        let cursor_at = self
            .lines
            .get(self.row)
            .map_or(0, |line| Self::byte_at(line, self.col));
        let mut rows: Vec<(usize, usize, usize)> = Vec::with_capacity(self.lines.len());
        let mut cursor_row = 0;
        for (index, line) in self.lines.iter().enumerate() {
            let first = rows.len();
            let mut start = 0;
            let mut used = 0;
            for (at, c) in line.char_indices() {
                let cells = c.width().unwrap_or(0);
                if used + cells > width && at > start {
                    rows.push((index, start, at));
                    start = at;
                    used = 0;
                }
                used += cells;
            }
            rows.push((index, start, line.len()));
            if index == self.row {
                if cursor_at == line.len() && used >= width {
                    rows.push((index, line.len(), line.len()));
                }
                cursor_row = rows
                    .get(first..)
                    .and_then(|own| own.iter().rposition(|(_, start, _)| *start <= cursor_at))
                    .map_or(first, |offset| first + offset);
            }
        }
        if cursor_row < self.scroll {
            self.scroll = cursor_row;
        } else if cursor_row >= self.scroll + height {
            self.scroll = cursor_row + 1 - height;
        }
        let masked = self.masked;
        let input = theme.input();
        let cursor = input.fg(theme.brand);
        let lines = &self.lines;
        rows.iter()
            .enumerate()
            .skip(self.scroll)
            .take(height)
            .map(|(visual, &(index, start, end))| {
                let text = lines
                    .get(index)
                    .and_then(|line| line.get(start..end))
                    .unwrap_or_default();
                if masked {
                    Line::styled(MASK.repeat(text.chars().count().max(1)), input)
                } else if visual == cursor_row {
                    cursor_line(text, cursor_at.saturating_sub(start), input, cursor)
                } else {
                    Line::styled(text, input)
                }
            })
            .collect()
    }
}

fn cursor_line(text: &str, at: usize, input: Style, cursor: Style) -> Line<'_> {
    let (before, after) = text.split_at_checked(at).unwrap_or((text, ""));
    let mut chars = after.chars();
    let under = chars.next();
    let rest = chars.as_str();
    let mut spans = vec![Span::styled(before, input)];
    match under {
        Some(c) => {
            spans.push(Span::styled(
                after.get(..c.len_utf8()).unwrap_or_default(),
                Style::new().patch(input).reversed(),
            ));
            spans.push(Span::styled(rest, input));
        }
        None => spans.push(Span::styled(CURSOR, cursor)),
    }
    Line::from(spans)
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/form/editor/textarea/tests.rs"]
mod tests;
