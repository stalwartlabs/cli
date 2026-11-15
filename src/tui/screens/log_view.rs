/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::app::{Cx, ScreenMsg, ViewCx};
use crate::tui::keymap::{Context, code, ctrl, key, keys, upper};
use crate::tui::log::{HttpLog, MAX_LINES};
use crate::tui::widgets::chrome::Crumbs;
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_millis(500);
const PAGE: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    PageDown,
    PageUp,
    Top,
    Bottom,
    Back,
}

keys! { pub KEYS, pub HINTS: Action;
    "j/k" Down "scroll" true [code(KeyCode::Down), key('j')],
    "k ↑" Up "scroll up" false [code(KeyCode::Up), key('k')],
    "^f" PageDown "page down" false [code(KeyCode::PageDown), ctrl('f')],
    "^b" PageUp "page up" false [code(KeyCode::PageUp), ctrl('b')],
    "g" Top "oldest" true [key('g'), code(KeyCode::Home)],
    "G" Bottom "follow newest" true [upper('G'), code(KeyCode::End)],
    "esc" Back "back" true [code(KeyCode::Esc), code(KeyCode::Backspace)],
}

pub struct State {
    log: HttpLog,
    seen: u64,
    lines: VecDeque<Box<str>>,
    top: usize,
    follow: bool,
    next_refresh: Instant,
}

impl State {
    pub fn new(log: HttpLog, now: Instant) -> Self {
        let mut state = State {
            log,
            seen: 0,
            lines: VecDeque::with_capacity(MAX_LINES),
            top: 0,
            follow: true,
            next_refresh: now + REFRESH,
        };
        state.log.copy_new(&mut state.seen, &mut state.lines);
        state
    }

    pub fn deadline(&self) -> Option<Instant> {
        Some(self.next_refresh)
    }

    pub fn refresh(&mut self, now: Instant) -> bool {
        if now < self.next_refresh {
            return false;
        }
        self.next_refresh = now + REFRESH;
        self.log.copy_new(&mut self.seen, &mut self.lines)
    }
}

pub fn context(_state: &State) -> Context {
    Context::Log
}

pub fn update(state: &mut State, cx: &mut Cx<'_>, msg: ScreenMsg<'_, Action>) {
    let ScreenMsg::Key(action) = msg else {
        return;
    };
    let last = state.lines.len().saturating_sub(1);
    state.follow = false;
    match action {
        Action::Down => state.top = (state.top + 1).min(last),
        Action::Up => state.top = state.top.saturating_sub(1),
        Action::PageDown => state.top = (state.top + PAGE).min(last),
        Action::PageUp => state.top = state.top.saturating_sub(PAGE),
        Action::Top => state.top = 0,
        Action::Bottom => state.follow = true,
        Action::Back => cx.back(),
    }
}

pub fn render(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    let block = theme.panel(" HTTP log ", cx.focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let height = usize::from(inner.height);
    if state.follow {
        state.top = state.lines.len().saturating_sub(height);
    }
    if state.lines.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(" No requests recorded yet.", theme.muted())),
            inner,
        );
        return;
    }
    let lines = state.lines.iter().skip(state.top).take(height).map(|line| {
        let style = if line.starts_with("<-") {
            theme.muted()
        } else {
            Style::new().fg(theme.accent)
        };
        Line::from(vec![Span::raw(" "), Span::styled(line.as_ref(), style)])
    });
    frame.render_widget(Paragraph::new(lines.collect::<Vec<_>>()), inner);
}

pub fn crumbs(_state: &State, out: &mut Crumbs) {
    out.push("HTTP log");
}
