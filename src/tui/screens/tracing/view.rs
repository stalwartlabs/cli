/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use super::{Adding, FormFocus, MAX_EVENTS, Run, State, Status};
use crate::tui::app::ViewCx;
use crate::tui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

const LABEL_WIDTH: u16 = 12;
const INPUT_WIDTH: u16 = 60;

pub fn render(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let area = area.inner(ratatui::layout::Margin::new(1, 0));
    match &mut state.run {
        Some(run) => render_run(run, &state.labels, frame, area, cx.theme),
        None => render_form(state, frame, area, cx),
    }
}

fn render_form(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    let [title, subtitle, _, keywords, filters, value, _, help] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(Line::styled("Live Tracing", theme.title()), title);
    frame.render_widget(
        Line::styled("Stream server events in real time.", theme.muted()),
        subtitle,
    );
    let idle = matches!(state.adding, Adding::Idle);
    let typing = idle && state.focus == FormFocus::Keywords;
    let [label, input] =
        Layout::horizontal([Constraint::Length(LABEL_WIDTH), Constraint::Fill(1)]).areas(keywords);
    frame.render_widget(Line::styled("Keywords", label_style(theme, typing)), label);
    let width = input.width.min(INPUT_WIDTH);
    frame.render_widget(
        state.keywords.line(theme, width, typing && cx.focused),
        input,
    );

    let selected = match state.focus {
        FormFocus::Filters(index) if idle => Some(index),
        _ => None,
    };
    let mut spans = vec![Span::styled(
        format!("{:<width$}", "Filters", width = usize::from(LABEL_WIDTH)),
        label_style(theme, selected.is_some()),
    )];
    if state.filters.is_empty() {
        spans.push(Span::styled("none (tab, then a to add one)", theme.muted()));
    }
    for (index, filter) in state.filters.iter().enumerate() {
        let style = if selected == Some(index) {
            theme.selected
        } else {
            theme.pill
        };
        spans.push(Span::styled(
            format!(" {} = {} ", filter.label, filter.value),
            style,
        ));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Line::from(spans), filters);

    if let Adding::Value { key, input } = &mut state.adding {
        let [label, field] =
            Layout::horizontal([Constraint::Length(LABEL_WIDTH), Constraint::Fill(1)]).areas(value);
        let text = state.labels.key_label(key);
        frame.render_widget(Line::styled(text.to_string(), theme.strong()), label);
        let width = field.width.min(INPUT_WIDTH);
        frame.render_widget(input.line(theme, width, cx.focused), field);
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                "Keywords match any event text; filters match one key exactly.",
                theme.muted(),
            ),
            Line::styled(
                "Press enter to start; stopping keeps the events on screen.",
                theme.muted(),
            ),
        ]),
        help,
    );
    if let Adding::Picking(picker) = &mut state.adding {
        picker.render(frame, area, theme);
    }
}

fn label_style(theme: &Theme, active: bool) -> Style {
    if active {
        theme.strong()
    } else {
        theme.muted()
    }
}

fn render_run(
    run: &mut Run,
    labels: &crate::tui::widgets::timeline::Labels,
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
) {
    let failed = matches!(run.status, Status::Failed(_));
    let [head, filter, error, body, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(!run.summary.is_empty())),
        Constraint::Length(u16::from(failed)),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let (dot, text, color) = match &run.status {
        Status::Connecting => ("●", "Connecting…", theme.info),
        Status::Streaming => ("●", "Streaming events…", theme.ok),
        Status::Reconnecting(_) => ("●", "Reconnecting…", theme.warn),
        Status::Stopped => ("■", "Stopped", theme.dim),
        Status::Failed(_) => ("✕", "Failed", theme.err),
    };
    let mut spans = vec![
        Span::styled("Live Tracing", theme.title()),
        Span::raw("  "),
        Span::styled(dot, Style::new().fg(color)),
        Span::styled(" ", Style::new()),
        Span::styled(text, Style::new().fg(color)),
    ];
    if let Status::Reconnecting(attempt) = run.status {
        spans.push(Span::styled(format!(" (attempt {attempt})"), theme.muted()));
    }
    frame.render_widget(Line::from(spans), head);
    if !run.summary.is_empty() {
        frame.render_widget(
            Line::from(vec![
                Span::styled("filter  ", theme.muted()),
                Span::styled(&*run.summary, theme.text()),
            ]),
            filter,
        );
    }
    if let Status::Failed(message) = &run.status {
        frame.render_widget(Line::styled(&**message, Style::new().fg(theme.err)), error);
    }
    let events = run.events.make_contiguous();
    run.timeline
        .render(frame, body, theme, labels, events, run.anchor);
    let mut count = format!(
        "{} event{}",
        events.len(),
        if events.len() == 1 { "" } else { "s" }
    );
    if events.len() >= MAX_EVENTS {
        count.push_str(" (buffer full)");
    }
    if !run.timeline.following() {
        count.push_str(" · scrolled, G to follow");
    }
    frame.render_widget(Line::styled(count, theme.muted()), foot);
}
