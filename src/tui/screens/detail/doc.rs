/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::expression::{Expression, KEYWORD_WIDTH};
use crate::tui::theme::Theme;
use crate::tui::widgets::chrome;
use crate::view::object::{DocBody, Row, RowValue};
use crate::view::{ObjectDoc, Styled, Table};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

const LABEL_WIDTH: usize = 26;
const INDENT: usize = 2;
const NOT_SET: &str = "not set";
const MIN_ROOM: usize = 16;

#[derive(Clone, Copy)]
struct Ctx<'a> {
    theme: &'a Theme,
    width: usize,
}

pub fn lines(doc: &ObjectDoc<'_>, theme: &Theme, width: usize, out: &mut Vec<Line<'static>>) {
    body(doc, Ctx { theme, width }, 0, out);
}

fn body(doc: &ObjectDoc<'_>, cx: Ctx<'_>, indent: usize, out: &mut Vec<Line<'static>>) {
    let theme = cx.theme;
    if let Some(variant) = doc.variant_label {
        out.push(Line::from(vec![
            pad(indent),
            Span::styled(variant.to_string(), theme.muted()),
        ]));
    }
    match &doc.body {
        DocBody::Raw(value) => {
            let text = serde_json::to_string_pretty(value).unwrap_or_default();
            out.extend(
                text.lines()
                    .map(|line| Line::from(vec![pad(indent), Span::raw(line.to_string())])),
            );
        }
        DocBody::Sections(sections) => {
            for section in sections {
                if let Some(title) = section.title {
                    if !out.is_empty() {
                        out.push(Line::raw(""));
                    }
                    out.push(Line::from(vec![
                        pad(indent),
                        Span::styled(format!("── {title} ──"), theme.title()),
                    ]));
                }
                for row in &section.rows {
                    self::row(row, cx, indent, out);
                }
            }
        }
    }
}

fn row(row: &Row<'_>, cx: Ctx<'_>, indent: usize, out: &mut Vec<Line<'static>>) {
    let theme = cx.theme;
    let width = LABEL_WIDTH.saturating_sub(indent);
    let label = Span::styled(format!("{:>width$}  ", row.label), theme.title());
    match &row.value {
        RowValue::Inline(styled) => {
            let column = indent + label.width();
            let room = cx.width.saturating_sub(column);
            if styled.width() <= room || room < MIN_ROOM {
                let mut spans = vec![pad(indent), label];
                owned(styled, theme, &mut spans);
                out.push(Line::from(spans));
                return;
            }
            let mut value = Vec::new();
            owned(styled, theme, &mut value);
            let mut first = Some(label);
            for spans in wrap(value, room) {
                let mut line = match first.take() {
                    Some(label) => vec![pad(indent), label],
                    None => vec![pad(column)],
                };
                line.extend(spans);
                out.push(Line::from(line));
            }
        }
        RowValue::Empty => out.push(Line::from(vec![
            pad(indent),
            label,
            Span::styled(NOT_SET, theme.muted()),
        ])),
        RowValue::Block(text) => {
            out.push(Line::from(vec![pad(indent), label]));
            out.extend(
                text.lines().map(|line| {
                    Line::from(vec![pad(indent + INDENT), Span::raw(line.to_string())])
                }),
            );
        }
        RowValue::Nested(doc) => {
            out.push(Line::from(vec![pad(indent), label]));
            body(doc, cx, indent + INDENT, out);
        }
        RowValue::Entries(entries) => {
            out.push(Line::from(vec![pad(indent), label]));
            for entry in entries {
                out.push(Line::from(vec![
                    pad(indent + INDENT),
                    Span::styled(entry.key.to_string(), theme.title()),
                ]));
                body(&entry.doc, cx, indent + INDENT * 2, out);
            }
        }
        RowValue::Table(table) => {
            out.push(Line::from(vec![pad(indent), label]));
            self::table(table, theme, indent + INDENT, out);
        }
        RowValue::Expression(expression) => {
            let column = indent + width + 2;
            self::expression(expression, cx, label, indent, column, out);
        }
    }
}

fn expression(
    expression: &Expression<&str>,
    cx: Ctx<'_>,
    label: Span<'static>,
    indent: usize,
    column: usize,
    out: &mut Vec<Line<'static>>,
) {
    let room = match cx.width.saturating_sub(column + KEYWORD_WIDTH) {
        room if room < MIN_ROOM => usize::MAX,
        room => room,
    };
    let mut label = Some(label);
    for (part, text) in expression.parts() {
        let keyword = part.keyword();
        let mut chunks = text
            .split('\n')
            .flat_map(|line| chrome::wrap_lines(line, room));
        let mut spans = match label.take() {
            Some(label) => vec![pad(indent), label],
            None => vec![pad(column)],
        };
        spans.push(Span::styled(
            format!("{keyword:<KEYWORD_WIDTH$}"),
            Style::new().fg(cx.theme.accent),
        ));
        spans.push(Span::raw(chunks.next().unwrap_or_default().to_owned()));
        out.push(Line::from(spans));
        out.extend(chunks.map(|chunk| {
            Line::from(vec![
                pad(column + KEYWORD_WIDTH),
                Span::raw(chunk.to_owned()),
            ])
        }));
    }
}

fn table(table: &Table<'_>, theme: &Theme, indent: usize, out: &mut Vec<Line<'static>>) {
    let mut widths: Vec<usize> = table.header.iter().map(|h| h.width()).collect();
    for row in &table.rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.width());
        }
    }
    let mut header = vec![pad(indent)];
    for (title, width) in table.header.iter().zip(&widths) {
        header.push(Span::styled(format!("{title:<width$}  "), theme.title()));
    }
    out.push(Line::from(header));
    for row in &table.rows {
        let mut spans = vec![pad(indent)];
        for (cell, width) in row.iter().zip(&widths) {
            owned(cell, theme, &mut spans);
            spans.push(pad(width.saturating_sub(cell.width()) + 2));
        }
        out.push(Line::from(spans));
    }
}

pub(super) fn wrap(spans: Vec<Span<'static>>, room: usize) -> Vec<Vec<Span<'static>>> {
    let mut text = String::new();
    let mut runs = Vec::with_capacity(spans.len());
    for span in &spans {
        let start = text.len();
        text.push_str(&span.content);
        runs.push((start, text.len(), span.style));
    }
    chrome::wrap_lines(&text, room)
        .map(|chunk| {
            let start = (chunk.as_ptr() as usize).saturating_sub(text.as_ptr() as usize);
            let end = start + chunk.len();
            runs.iter()
                .filter_map(|&(from, to, style)| {
                    let piece = text.get(from.max(start)..to.min(end))?;
                    (!piece.is_empty()).then(|| Span::styled(piece.to_owned(), style))
                })
                .collect()
        })
        .collect()
}

fn owned(styled: &Styled, theme: &Theme, spans: &mut Vec<Span<'static>>) {
    spans.extend(
        theme
            .spans(styled)
            .map(|span| Span::styled(span.content.into_owned(), span.style)),
    );
}

fn pad(width: usize) -> Span<'static> {
    Span::raw(chrome::pad(width))
}
