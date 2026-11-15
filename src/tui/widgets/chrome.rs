/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Error;
use crate::jmap::account::Edition;
use crate::tui::keymap::{Context, Hint, global};
use crate::tui::session::edition_label;
use crate::tui::theme::{Level, Theme};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, Paragraph};
use std::iter;
use std::time::{Duration, Instant};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SPACES: &str = "                                                                ";
const HINT_GAP: &str = "  ";
const CRUMB_SEP: &str = " › ";
const HEADER_SEP: &str = " │ ";
const LOGO_PILL: &str = " ◆ STALWART ";
const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"), " ");
const TRIAL_MARKER: &str = "★ Try Enterprise";
const HELP_WIDTH: u16 = 96;
const HELP_HEIGHT: u16 = 30;
const HELP_KEY_COLUMN: usize = 12;
const FLASH_TTL: Duration = Duration::from_secs(4);
const FLASH_ERROR_TTL: Duration = Duration::from_secs(8);
const SIDEBAR_HINT: Hint = Hint {
    label: "←",
    desc: "sidebar",
    footer: true,
};

pub fn pad(columns: usize) -> &'static str {
    SPACES.get(..columns.min(SPACES.len())).unwrap_or_default()
}

pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    area.centered(
        Constraint::Length(width.min(area.width)),
        Constraint::Length(height.min(area.height)),
    )
}

pub fn clear_popup(frame: &mut Frame, area: Rect, theme: &Theme) {
    frame.render_widget(Clear, area);
    frame.render_widget(Block::new().style(theme.popup()), area);
}

pub struct Flash {
    pub level: Level,
    pub text: Box<str>,
    pub expires: Instant,
}

#[derive(Default)]
pub struct FlashSlot {
    current: Option<Flash>,
}

impl FlashSlot {
    pub fn set(&mut self, level: Level, text: impl Into<Box<str>>, now: Instant) {
        let ttl = match level {
            Level::Error | Level::Warn => FLASH_ERROR_TTL,
            Level::Info | Level::Ok => FLASH_TTL,
        };
        self.current = Some(Flash {
            level,
            text: text.into(),
            expires: now + ttl,
        });
    }

    pub fn get(&self) -> Option<&Flash> {
        self.current.as_ref()
    }

    pub fn expire(&mut self, now: Instant) -> bool {
        if self.current.as_ref().is_some_and(|f| f.expires <= now) {
            self.current = None;
            return true;
        }
        false
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.current.as_ref().map(|f| f.expires)
    }
}

#[derive(Default)]
pub struct Crumbs {
    parts: Vec<String>,
    len: usize,
}

impl Crumbs {
    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn push(&mut self, part: &str) {
        match self.parts.get_mut(self.len) {
            Some(slot) => {
                slot.clear();
                slot.push_str(part);
            }
            None => self.parts.push(part.to_string()),
        }
        self.len += 1;
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.parts.iter().take(self.len).map(String::as_str)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Ok,
    Degraded,
    Down,
}

impl Health {
    pub fn after(error: Option<&Error>) -> Self {
        match error {
            Some(Error::Network(_)) => Health::Down,
            _ => Health::Ok,
        }
    }
}

pub struct HeaderInfo<'a> {
    pub server: &'a str,
    pub account: &'a str,
    pub edition: Edition,
    pub health: Health,
}

pub fn header(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    info: &HeaderInfo<'_>,
    crumbs: &Crumbs,
) {
    let [top, bottom] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let pill = Style::new()
        .bg(theme.brand)
        .fg(theme.brand_fg)
        .add_modifier(Modifier::BOLD);
    let health = match info.health {
        Health::Ok => theme.ok,
        Health::Degraded => theme.warn,
        Health::Down => theme.err,
    };
    let mut left = Line::from(vec![
        Span::styled(LOGO_PILL, pill),
        Span::raw(" "),
        Span::styled(info.server, theme.title()),
        Span::styled(HEADER_SEP, theme.muted()),
        Span::styled(info.account, theme.text()),
    ]);
    left.push_span(Span::styled(HEADER_SEP, theme.muted()));
    left.push_span(Span::styled(edition_label(info.edition), theme.muted()));
    left.push_span(Span::styled(HEADER_SEP, theme.muted()));
    left.push_span(Span::styled("●", Style::new().fg(health)));
    let mut right = Line::default();
    if info.edition == Edition::Community {
        right.push_span(Span::styled(
            TRIAL_MARKER,
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
        right.push_span(Span::styled(HEADER_SEP, theme.muted()));
    }
    right.push_span(Span::styled(VERSION, theme.muted()));
    let right_width = u16::try_from(right.width()).unwrap_or(0).min(top.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_width)]).areas(top);
    frame.render_widget(Paragraph::new(left), left_area);
    frame.render_widget(Paragraph::new(right.right_aligned()), right_area);
    frame.render_widget(Paragraph::new(crumb_line(theme, crumbs)), bottom);
}

fn crumb_line<'a>(theme: &Theme, crumbs: &'a Crumbs) -> Line<'a> {
    let mut line = Line::from(Span::raw(" "));
    let last = crumbs.len.saturating_sub(1);
    for (i, crumb) in crumbs.iter().enumerate() {
        if i > 0 {
            line.push_span(Span::styled(CRUMB_SEP, theme.muted()));
        }
        let style = if i == last {
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            theme.muted()
        };
        line.push_span(Span::styled(crumb, style));
    }
    line
}

pub struct FooterInfo<'a> {
    pub ctx: Context,
    pub available: &'a dyn Fn(usize) -> bool,
    pub sidebar_hidden: bool,
}

pub fn help_key(ctx: Context) -> &'static str {
    if ctx.types_question_mark() { "F1" } else { "?" }
}

pub fn footer(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    info: &FooterInfo<'_>,
    flash: Option<&Flash>,
) {
    let ctx = info.ctx;
    let name = ctx.name();
    let flash_width = flash.map_or(0, |f| f.text.width() + 3);
    let budget = usize::from(area.width).saturating_sub(flash_width + name.width() + 3);
    let help_label = help_key(ctx);
    let help = hint_width(help_label, "help");
    let mut line = Line::from(vec![
        Span::styled(" ", theme.pill),
        Span::styled(name, theme.pill),
        Span::styled(" ", theme.pill),
        Span::raw(" "),
    ]);
    let mut used = 0;
    let sidebar = (info.sidebar_hidden && !ctx.captures_text()).then_some(SIDEBAR_HINT);
    let hints = sidebar.as_ref().into_iter().chain(
        ctx.hints()
            .iter()
            .enumerate()
            .filter(|(index, hint)| hint.footer && (info.available)(*index))
            .map(|(_, hint)| hint),
    );
    for hint in hints {
        let width = hint_width(hint.label, hint.desc);
        if used + width + help > budget {
            break;
        }
        used += width;
        push_hint(&mut line, theme, hint.label, hint.desc);
    }
    if used + help <= budget {
        push_hint(&mut line, theme, help_label, "help");
    }
    frame.render_widget(Paragraph::new(line), area);
    if let Some(flash) = flash {
        let color = theme.level(flash.level);
        let icon = match flash.level {
            Level::Info => "●",
            Level::Ok => "✓",
            Level::Warn => "!",
            Level::Error => "✗",
        };
        let line = Line::from(vec![
            Span::styled(icon, Style::new().fg(color).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(flash.text.as_ref(), Style::new().fg(color)),
            Span::raw(" "),
        ]);
        frame.render_widget(Paragraph::new(line.right_aligned()), area);
    }
}

fn hint_width(label: &str, desc: &str) -> usize {
    label.width() + 1 + desc.width() + HINT_GAP.len()
}

fn push_hint<'a>(line: &mut Line<'a>, theme: &Theme, label: &'a str, desc: &'a str) {
    line.push_span(Span::styled(label, theme.key));
    line.push_span(Span::styled(" ", theme.muted()));
    line.push_span(Span::styled(desc, theme.muted()));
    line.push_span(Span::raw(HINT_GAP));
}

pub fn help_overlay(frame: &mut Frame, theme: &Theme, ctx: Context) {
    let screen = frame.area();
    let above_footer = Rect {
        height: screen.height.saturating_sub(1),
        ..screen
    };
    let area = centered(above_footer, HELP_WIDTH, HELP_HEIGHT);
    clear_popup(frame, area, theme);
    let block = theme.popup_block(" Keys ", theme.accent).title_bottom(
        Line::from(" any key closes ")
            .right_aligned()
            .style(theme.muted()),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
            .spacing(2)
            .areas(inner);
    let key_column = HELP_KEY_COLUMN.min(usize::from(left.width) / 3);
    let mut text = hint_lines(theme, ctx.name(), ctx.hints(), key_column, left.width);
    if ctx.types_question_mark() {
        key_lines(
            &mut text,
            theme,
            "F1",
            "help (? is typed here)",
            key_column,
            left.width,
        );
    }
    frame.render_widget(Paragraph::new(text), left);
    let key_column = HELP_KEY_COLUMN.min(usize::from(right.width) / 3);
    let mut text = hint_lines(theme, "Global", global::HINTS, key_column, right.width);
    text.lines.push(Line::raw(""));
    text.lines.push(Line::styled("Conventions", theme.title()));
    for (key, desc) in [
        ("vim + arrows", "both work everywhere"),
        ("capital Y", "confirms destructive actions"),
        (
            "esc",
            "closes the topmost layer, goes back, or focuses the sidebar",
        ),
        ("^x", "means Ctrl+x, S- means Shift"),
        ("^u ^a ^e", "clear, start, end of a text field"),
        ("tab", "next unit while editing a size or duration"),
    ] {
        key_lines(&mut text, theme, key, desc, key_column, right.width);
    }
    frame.render_widget(Paragraph::new(text), right);
}

fn key_lines<'a>(
    text: &mut Text<'a>,
    theme: &Theme,
    key: &'a str,
    desc: &'a str,
    key_column: usize,
    width: u16,
) {
    let indent = key_column.max(key.width()) + HINT_GAP.len();
    let room = usize::from(width).saturating_sub(indent).max(1);
    let mut rest = desc;
    let mut first = true;
    while first || !rest.is_empty() {
        let (chunk, tail) = wrap_at(rest, room);
        let lead = if first {
            vec![
                Span::raw(pad(key_column.saturating_sub(key.width()))),
                Span::styled(key, theme.key),
                Span::raw(HINT_GAP),
            ]
        } else {
            vec![Span::raw(pad(indent))]
        };
        let mut line = Line::from(lead);
        line.push_span(Span::styled(chunk, theme.text()));
        text.lines.push(line);
        rest = tail;
        first = false;
    }
}

pub fn wrap_lines(text: &str, width: usize) -> impl Iterator<Item = &str> {
    let mut rest = Some(text);
    iter::from_fn(move || {
        let (head, tail) = wrap_at(rest?, width);
        rest = (!tail.is_empty()).then_some(tail);
        Some(head)
    })
}

fn wrap_at(text: &str, room: usize) -> (&str, &str) {
    let mut used = 0;
    let mut space = None;
    for (index, c) in text.char_indices() {
        used += c.width().unwrap_or(0);
        if used > room {
            let split = match space {
                Some(space) => text
                    .split_at_checked(space)
                    .map(|(head, tail)| (head, tail.strip_prefix(' ').unwrap_or(tail))),
                None => text.split_at_checked(index.max(c.len_utf8())),
            };
            return split.unwrap_or((text, ""));
        }
        if c == ' ' && index > 0 {
            space = Some(index);
        }
    }
    (text, "")
}

fn hint_lines<'a>(
    theme: &Theme,
    title: &'a str,
    hints: &'a [Hint],
    key_column: usize,
    width: u16,
) -> Text<'a> {
    let mut text = Text::from(Vec::with_capacity(hints.len() + 2));
    text.lines.push(Line::styled(title, theme.title()));
    text.lines.push(Line::raw(""));
    for hint in hints {
        key_lines(&mut text, theme, hint.label, hint.desc, key_column, width);
    }
    text
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/widgets/chrome/tests.rs"]
mod tests;
