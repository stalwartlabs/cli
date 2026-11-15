/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use super::stage::{body, details};
use super::{INVALID_TARGET, Phase, Run, State, Status};
use crate::jmap::live::{Severity, StageKind};
use crate::tui::app::ViewCx;
use crate::tui::theme::Theme;
use crate::tui::widgets::spinner;
use crate::tui::widgets::timeline::write_elapsed;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::fmt::Write;

const LABEL_WIDTH: u16 = 26;
const INPUT_WIDTH: u16 = 50;

pub fn render(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let area = area.inner(Margin::new(1, 0));
    let details = state.details;
    match &mut state.run {
        Some(run) => render_run(run, details, frame, area, cx),
        None => render_form(state, frame, area, cx),
    }
}

fn render_form(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    let [title, subtitle, _, field, error, _, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    frame.render_widget(Line::styled("Delivery Trace", theme.title()), title);
    frame.render_widget(
        Paragraph::new(
            "Run a real outbound SMTP delivery attempt and watch every DNS lookup, TLS handshake, and SMTP command as it happens.",
        )
        .style(theme.muted()),
        subtitle,
    );
    let [label, input] =
        Layout::horizontal([Constraint::Length(LABEL_WIDTH), Constraint::Fill(1)]).areas(field);
    frame.render_widget(
        Line::styled("Target address or domain", theme.strong()),
        label,
    );
    let width = input.width.min(INPUT_WIDTH);
    frame.render_widget(
        state.target.line(theme, width, cx.focused && state.editing),
        input,
    );
    if state.invalid {
        frame.render_widget(
            Line::styled(INVALID_TARGET, Style::new().fg(theme.err)),
            error,
        );
    }
    frame.render_widget(
        Line::styled(
            if state.editing {
                "e.g. john@example.org or example.org; enter starts the trace"
            } else {
                "press enter to type a target"
            },
            theme.muted(),
        ),
        hint,
    );
}

fn render_run(run: &mut Run, details: bool, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    let [head, summary, _, body_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    let elapsed = run
        .finished
        .unwrap_or(cx.now)
        .saturating_duration_since(run.started);
    let mut title = String::with_capacity(64);
    let summary_line = match &run.status {
        Status::Running => {
            let _ = write!(title, "Tracing delivery to {}", run.target);
            let secs = elapsed.as_secs();
            let started = if secs < 60 {
                format!("Started {secs}s ago")
            } else {
                format!("Started {}m {}s ago", secs / 60, secs % 60)
            };
            Line::styled(started, theme.muted())
        }
        Status::Completed => {
            let _ = write!(title, "Trace completed for {}", run.target);
            let (pill, color) = if run.delivered {
                ("Delivery successful", theme.ok)
            } else {
                ("Delivery failed", theme.err)
            };
            let mut text = format!(
                "  {} attempt{} in ",
                run.attempts,
                if run.attempts == 1 { "" } else { "s" }
            );
            write_elapsed(
                &mut text,
                i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
            );
            Line::from(vec![
                Span::styled(format!(" {pill} "), Style::new().fg(theme.bg).bg(color)),
                Span::styled(text, theme.text()),
            ])
        }
        Status::Failed(message) => {
            let _ = write!(title, "Trace failed for {}", run.target);
            Line::styled(message.to_string(), Style::new().fg(theme.err))
        }
    };
    frame.render_widget(Line::styled(title, theme.title()), head);
    frame.render_widget(summary_line, summary);

    let mut lines = Vec::with_capacity(run.rows.len() + 2);
    let mut extra = Vec::new();
    let mut in_attempt = false;
    for row in &run.rows {
        let stage = &row.stage;
        let attempt = row.phase == Phase::Attempt;
        if attempt {
            in_attempt = true;
        }
        let indent = if in_attempt && !attempt { "  │ " } else { "" };
        let severity = if attempt {
            Severity::Ok
        } else {
            stage.kind.severity()
        };
        let (icon, color) = match severity {
            Severity::Pending => (spinner::frame(cx.tick), theme.info),
            Severity::Ok => ("✓", theme.ok),
            Severity::Warn => ("▲", theme.warn),
            Severity::Error => ("✕", theme.err),
        };
        let mut spans = vec![
            Span::styled(indent, theme.muted()),
            Span::styled(icon, Style::new().fg(color)),
            Span::raw(" "),
        ];
        if attempt {
            spans.push(Span::styled(
                format!("Delivery Attempt: {}", stage.hostname().unwrap_or("?")),
                theme.strong(),
            ));
        } else {
            let label = match stage.kind {
                StageKind::Unknown => "Unknown stage",
                _ => row.phase.label(),
            };
            spans.push(Span::styled(label, theme.strong()));
        }
        if let Some(ms) = stage.elapsed_ms() {
            let mut text = String::from("  ");
            write_elapsed(&mut text, i64::try_from(ms).unwrap_or(i64::MAX));
            spans.push(Span::styled(text, theme.muted()));
        }
        if let Some(text) = body(stage) {
            let style = if severity == Severity::Error {
                Style::new().fg(theme.err)
            } else {
                theme.muted()
            };
            spans.push(Span::raw("  "));
            spans.push(Span::styled(text.into_owned(), style));
        }
        lines.push(Line::from(spans));
        if details {
            details_lines(stage, indent, theme, &mut extra, &mut lines);
        }
    }
    if run.status == Status::Running {
        lines.push(Line::styled(
            format!("{} Waiting for more events", spinner::frame(cx.tick)),
            theme.muted(),
        ));
    }
    let max_scroll = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_sub(body_area.height);
    run.scroll = run.scroll.min(max_scroll);
    frame.render_widget(Paragraph::new(lines).scroll((run.scroll, 0)), body_area);
}

fn details_lines(
    stage: &crate::jmap::live::DeliveryStage,
    indent: &'static str,
    theme: &Theme,
    scratch: &mut Vec<String>,
    lines: &mut Vec<Line<'static>>,
) {
    scratch.clear();
    details(stage, scratch);
    for text in scratch.drain(..) {
        lines.push(Line::from(vec![
            Span::styled(indent, theme.muted()),
            Span::styled("    ", theme.muted()),
            Span::styled(text, theme.text()),
        ]));
    }
}
