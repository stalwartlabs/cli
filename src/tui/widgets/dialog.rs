/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::keymap::{Context, Hint};
use crate::tui::theme::Theme;
use crate::tui::widgets::chrome::{centered, clear_popup};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use std::mem;

const WIDTH: u16 = 74;
const CHROME_ROWS: u16 = 6;
const INNER_PAD: u16 = 6;

pub const INFO_HINTS: &[Hint] = &[Hint {
    label: "⏎ esc",
    desc: "close",
    footer: true,
}];

pub const COPY_HINTS: &[Hint] = &[
    Hint {
        label: "c",
        desc: "copy and close",
        footer: true,
    },
    Hint {
        label: "esc",
        desc: "close",
        footer: true,
    },
];

pub const HINTS: &[Hint] = &[
    Hint {
        label: "Y",
        desc: "confirm (capital Y)",
        footer: true,
    },
    Hint {
        label: "N",
        desc: "cancel",
        footer: true,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Risk {
    Low,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialogKey {
    pub key: char,
    pub label: &'static str,
    pub danger: bool,
}

pub const CONFIRM_DESTROY: &[DialogKey] = &[DialogKey {
    key: 'Y',
    label: "delete (capital Y)",
    danger: true,
}];

pub const CONFIRM_APPLY: &[DialogKey] = &[DialogKey {
    key: 'Y',
    label: "apply (capital Y)",
    danger: false,
}];

pub const CONFIRM_RUN: &[DialogKey] = &[DialogKey {
    key: 'Y',
    label: "run (capital Y)",
    danger: false,
}];

pub const CONFIRM_DISCARD: &[DialogKey] = &[DialogKey {
    key: 'Y',
    label: "discard (capital Y)",
    danger: true,
}];

pub const CONFIRM_LEAVE: &[DialogKey] = &[DialogKey {
    key: 'Y',
    label: "leave and cancel (capital Y)",
    danger: true,
}];

pub const COPY: &[DialogKey] = &[DialogKey {
    key: 'c',
    label: "copy and close",
    danger: false,
}];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Cancel,
    Key(char),
}

#[derive(Debug, Clone)]
pub struct Dialog {
    title: Box<str>,
    risk: Option<Risk>,
    body: Vec<Line<'static>>,
    keys: &'static [DialogKey],
    alternate: Option<(Vec<Line<'static>>, &'static str)>,
    scroll: u16,
}

impl Dialog {
    pub fn confirm(
        title: impl Into<Box<str>>,
        risk: Risk,
        body: Vec<Line<'static>>,
        keys: &'static [DialogKey],
    ) -> Self {
        Self {
            title: title.into(),
            risk: Some(risk),
            body,
            keys,
            alternate: None,
            scroll: 0,
        }
    }

    pub fn info(title: impl Into<Box<str>>, body: Vec<Line<'static>>) -> Self {
        Self {
            title: title.into(),
            risk: None,
            body,
            keys: &[],
            alternate: None,
            scroll: 0,
        }
    }

    pub fn set_body(&mut self, body: Vec<Line<'static>>) {
        self.body = body;
        self.scroll = 0;
    }

    pub fn with_keys(mut self, keys: &'static [DialogKey]) -> Self {
        self.keys = keys;
        self
    }

    pub fn with_alternate(mut self, body: Vec<Line<'static>>, label: &'static str) -> Self {
        self.alternate = Some((body, label));
        self
    }

    pub fn context(&self) -> Context {
        match (self.risk, self.keys.is_empty()) {
            (Some(_), _) => Context::Dialog,
            (None, true) => Context::DialogInfo,
            (None, false) => Context::Created,
        }
    }

    pub fn handle(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Char(c) if self.keys.iter().any(|k| k.key == c) => Outcome::Key(c),
            KeyCode::Tab | KeyCode::Char('J') if self.alternate.is_some() => {
                if let Some((body, _)) = &mut self.alternate {
                    mem::swap(&mut self.body, body);
                    self.scroll = 0;
                }
                Outcome::Pending
            }
            KeyCode::Esc | KeyCode::Char('n' | 'N' | 'q') => Outcome::Cancel,
            KeyCode::Enter if self.keys.is_empty() => Outcome::Cancel,
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                Outcome::Pending
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                Outcome::Pending
            }
            _ => Outcome::Pending,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let inner_width = usize::from(WIDTH.min(area.width).saturating_sub(INNER_PAD)).max(1);
        let body_rows: usize = self
            .body
            .iter()
            .map(|line| line.width().div_ceil(inner_width).max(1))
            .sum();
        let body_rows = u16::try_from(body_rows).unwrap_or(u16::MAX);
        let popup = centered(area, WIDTH, body_rows.saturating_add(CHROME_ROWS));
        clear_popup(frame, popup, theme);
        let accent = match self.risk {
            Some(Risk::High) => theme.err,
            Some(Risk::Low) => theme.ok,
            None => theme.accent,
        };
        let title = Line::from(vec![
            Span::raw(" "),
            Span::raw(&*self.title),
            Span::raw(" "),
        ]);
        let mut block = theme.popup_block(title, accent);
        if let Some(risk) = self.risk {
            block = block.title(risk_badge(theme, risk).right_aligned());
        }
        let inner = block.inner(popup).inner(Margin::new(2, 1));
        frame.render_widget(block, popup);

        let [body, footer_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
        let mut footer = Vec::new();
        for key in self.keys {
            let style = if key.danger {
                Style::new()
                    .bg(theme.err)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                theme.selected
            };
            footer.push(Span::styled(format!(" {} ", key.key), style));
            footer.push(Span::styled(format!(" {}     ", key.label), theme.text()));
        }
        if let Some((_, label)) = &self.alternate {
            footer.push(Span::styled(" J ", theme.selected));
            footer.push(Span::styled(format!(" {label}     "), theme.text()));
        }
        let (close_key, close) = match (self.keys.is_empty(), self.risk) {
            (true, _) => (" ⏎ ", " close"),
            (false, None) => (" esc ", " close"),
            (false, Some(_)) => (" N ", " cancel"),
        };
        footer.push(Span::styled(close_key, theme.selected));
        footer.push(Span::styled(close, theme.text()));
        frame.render_widget(
            Paragraph::new(self.body.clone())
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            body,
        );
        frame.render_widget(Line::from(footer), footer_area);
    }
}

pub fn risk_badge(theme: &Theme, risk: Risk) -> Line<'static> {
    let (text, bg) = match risk {
        Risk::High => (" HIGH RISK ", theme.err),
        Risk::Low => (" LOW RISK ", theme.ok),
    };
    Line::styled(
        text,
        Style::new()
            .bg(bg)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    )
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/widgets/dialog/tests.rs"]
mod tests;
