/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::expression::{KEYWORD_WIDTH, Part};
use crate::tui::form::expression::ExpressionView;
use crate::tui::form::render::{EDIT_HEIGHT, FormFrame, owned_line};
use crate::tui::widgets::chrome::{self, pad};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

const HINT: &str = " ⏎ edit  a add rule  d remove  J/K move  x clear  esc done ";
const READ_ONLY_HINT: &str = " j/k move  esc back ";
const ERROR_MARK: &str = "✗ ";
const ERROR_CONTINUE: &str = "  ";
const NO_RULES: &str = " No conditions · a adds one";
const VALUE_KEYWORD: &str = "value ";
const IF_KEYWORD: &str = "if    ";
const THEN_KEYWORD: &str = "then  ";
const ELSE_KEYWORD: &str = "else  ";
const CONDITION_PLACEHOLDER: &str = "condition";
const RESULT_PLACEHOLDER: &str = "result";
const VALUE_PLACEHOLDER: &str = "not set";
const DEFAULT_PLACEHOLDER: &str = "default value";
const NUMBER_WIDTH: usize = 5;
const GUTTER: usize = 2 + NUMBER_WIDTH;
const KEYWORD: usize = KEYWORD_WIDTH + 1;
const _: () = assert!(
    VALUE_KEYWORD.len() == KEYWORD
        && IF_KEYWORD.len() == KEYWORD
        && THEN_KEYWORD.len() == KEYWORD
        && ELSE_KEYWORD.len() == KEYWORD
);
const INDENT: usize = GUTTER + KEYWORD;

pub fn render(view: &mut ExpressionView, frame: &mut Frame, area: Rect, cx: &FormFrame<'_>) {
    let theme = cx.theme;
    let mut title = vec![Span::raw(" "), Span::raw(cx.title), Span::raw(" ")];
    if view.changed() {
        title.push(Span::styled("● unsaved ", Style::new().fg(theme.warn)));
    }
    let hint = if view.is_read_only() {
        READ_ONLY_HINT
    } else {
        HINT
    };
    let block = theme
        .panel(Line::from(title), cx.focused)
        .title_bottom(Line::styled(hint, theme.muted()).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = usize::from(inner.width).saturating_sub(INDENT).max(1);
    let mut editor: Vec<Line<'static>> = match view.editor() {
        Some(area) => area
            .wrapped(theme, width, EDIT_HEIGHT)
            .into_iter()
            .map(owned_line)
            .collect(),
        None => Vec::new(),
    };
    let error = Style::new().fg(theme.err);
    let rules = view.expression.rules.len();
    let mut lines: Vec<Line<'_>> = Vec::with_capacity(rules * 3 + 3);
    if let Some(status) = view.status {
        lines.push(Line::styled(format!(" {status}"), error));
    }
    if let Some(problem) = view.problem.as_ref().filter(|problem| problem.at.is_none()) {
        error_lines(
            &mut lines,
            1,
            &problem.message,
            usize::from(inner.width),
            error,
        );
    }
    if rules == 0 {
        lines.push(Line::styled(NO_RULES, theme.muted()));
    }
    let (mut focus_start, mut focus_end) = (0, 0);
    for (part, text) in view.expression.parts() {
        if matches!(part, Part::Condition(index) if index > 0)
            || (part == Part::Default && rules > 0)
        {
            lines.push(Line::raw(""));
        }
        let focused = part == view.cursor;
        let problem = view
            .problem
            .as_ref()
            .filter(|problem| problem.at == Some(part));
        if focused {
            focus_start = lines.len();
        }
        let number = match part {
            Part::Condition(index) => Span::styled(format!("{:>3}. ", index + 1), theme.muted()),
            Part::Result(_) | Part::Default => Span::raw(pad(NUMBER_WIDTH)),
        };
        let mut lead = Some(vec![
            Span::styled(
                if focused { "▌" } else { " " },
                Style::new().fg(theme.brand),
            ),
            match problem {
                Some(_) => Span::styled("✗", error),
                None => Span::raw(" "),
            },
            number,
            Span::styled(keyword(part, rules), Style::new().fg(theme.accent)),
        ]);
        let mut next_lead = || lead.take().unwrap_or_else(|| vec![Span::raw(pad(INDENT))]);
        if focused && !editor.is_empty() {
            for line in editor.drain(..) {
                let mut spans = next_lead();
                spans.extend(line.spans);
                lines.push(Line::from(spans));
            }
        } else if text.is_empty() {
            let mut spans = next_lead();
            spans.push(Span::styled(
                placeholder(part, rules),
                theme.muted().add_modifier(Modifier::ITALIC),
            ));
            lines.push(Line::from(spans));
        } else {
            let style = if focused {
                theme.selected
            } else {
                theme.text()
            };
            for chunk in text
                .split('\n')
                .flat_map(|line| chrome::wrap_lines(line, width))
            {
                let mut spans = next_lead();
                spans.push(Span::styled(chunk, style));
                lines.push(Line::from(spans));
            }
        }
        if let Some(problem) = problem {
            error_lines(
                &mut lines,
                INDENT,
                &problem.message,
                usize::from(inner.width),
                error,
            );
        }
        if focused {
            focus_end = lines.len();
        }
    }
    let height = usize::from(inner.height);
    let offset = focus_end.saturating_sub(height).min(focus_start);
    let scroll = u16::try_from(offset).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

fn keyword(part: Part, rules: usize) -> &'static str {
    match part {
        Part::Condition(_) => IF_KEYWORD,
        Part::Result(_) => THEN_KEYWORD,
        Part::Default if rules == 0 => VALUE_KEYWORD,
        Part::Default => ELSE_KEYWORD,
    }
}

fn placeholder(part: Part, rules: usize) -> &'static str {
    match part {
        Part::Condition(_) => CONDITION_PLACEHOLDER,
        Part::Result(_) => RESULT_PLACEHOLDER,
        Part::Default if rules == 0 => VALUE_PLACEHOLDER,
        Part::Default => DEFAULT_PLACEHOLDER,
    }
}

pub fn error_lines<'a>(
    lines: &mut Vec<Line<'a>>,
    indent: usize,
    message: &'a str,
    width: usize,
    style: Style,
) {
    let room = width.saturating_sub(indent + ERROR_MARK.width()).max(1);
    let mut mark = Some(ERROR_MARK);
    for chunk in message
        .split('\n')
        .flat_map(|line| chrome::wrap_lines(line, room))
    {
        lines.push(Line::from(vec![
            Span::raw(pad(indent)),
            Span::styled(mark.take().unwrap_or(ERROR_CONTINUE), style),
            Span::styled(chunk, style),
        ]));
    }
}
