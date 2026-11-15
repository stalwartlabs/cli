/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

const BULLET: &str = "  • ";

pub fn render(source: &str, theme: &Theme, out: &mut Vec<Line<'static>>) {
    let mut paragraph = String::new();
    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() {
            flush(&mut paragraph, theme, out);
            if out.last().is_some_and(|last| last.width() > 0) {
                out.push(Line::default());
            }
            continue;
        }
        if let Some(item) = list_item(line) {
            flush(&mut paragraph, theme, out);
            let mut spans = vec![Span::styled(item.marker, theme.muted())];
            inline(item.text, theme.text(), theme, &mut spans);
            out.push(Line::from(spans));
            continue;
        }
        if let Some(heading) = heading(line) {
            flush(&mut paragraph, theme, out);
            let mut spans = Vec::new();
            inline(heading, theme.title(), theme, &mut spans);
            out.push(Line::from(spans));
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(line);
    }
    flush(&mut paragraph, theme, out);
    while out.last().is_some_and(|last| last.width() == 0) {
        out.pop();
    }
}

fn flush(paragraph: &mut String, theme: &Theme, out: &mut Vec<Line<'static>>) {
    if paragraph.is_empty() {
        return;
    }
    let mut spans = Vec::new();
    inline(paragraph, theme.text(), theme, &mut spans);
    out.push(Line::from(spans));
    paragraph.clear();
}

struct ListItem<'a> {
    marker: String,
    text: &'a str,
}

fn list_item(line: &str) -> Option<ListItem<'_>> {
    if let Some(text) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
    {
        return Some(ListItem {
            marker: BULLET.to_string(),
            text,
        });
    }
    let (number, text) = line.split_once(". ")?;
    if number.is_empty() || number.len() > 3 || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(ListItem {
        marker: format!("  {number}. "),
        text,
    })
}

fn heading(line: &str) -> Option<&str> {
    let text = line.trim_start_matches('#');
    (text.len() < line.len() && line.len() - text.len() <= 6)
        .then(|| text.strip_prefix(' '))
        .flatten()
}

fn inline(text: &str, base: Style, theme: &Theme, spans: &mut Vec<Span<'static>>) {
    let code = Style::new().fg(theme.accent);
    let link = Style::new()
        .fg(theme.accent)
        .add_modifier(Modifier::UNDERLINED);
    let mut rest = text;
    while !rest.is_empty() {
        let Some((before, after)) = rest.split_once(['`', '*', '_', '[']) else {
            push(spans, rest, base);
            break;
        };
        let marker = rest.as_bytes().get(before.len()).copied();
        let parsed = match marker {
            Some(b'`') => after
                .split_once('`')
                .map(|(inner, tail)| (inner, code, None, tail)),
            Some(b'*') => match after.strip_prefix('*') {
                Some(bold) => bold
                    .split_once("**")
                    .map(|(inner, tail)| (inner, base.add_modifier(Modifier::BOLD), None, tail)),
                None => emphasis(after, '*', base),
            },
            Some(b'_') if before.is_empty() || before.ends_with(' ') => emphasis(after, '_', base),
            Some(b'[') => after.split_once("](").and_then(|(label, tail)| {
                let (url, tail) = tail.split_once(')')?;
                Some((label, link, Some(url), tail))
            }),
            _ => None,
        };
        match parsed {
            Some((inner, style, url, tail)) if !inner.is_empty() => {
                push(spans, before, base);
                push(spans, inner, style);
                if let Some(url) = url.filter(|url| *url != inner) {
                    push(spans, " <", theme.muted());
                    push(spans, url, theme.muted());
                    push(spans, ">", theme.muted());
                }
                rest = tail;
            }
            _ => {
                let consumed = rest.len() - after.len();
                push(spans, rest.get(..consumed).unwrap_or(rest), base);
                rest = after;
            }
        }
    }
}

fn emphasis(after: &str, marker: char, base: Style) -> Option<(&str, Style, Option<&str>, &str)> {
    if after.starts_with(' ') {
        return None;
    }
    after
        .split_once(marker)
        .filter(|(inner, _)| !inner.ends_with(' '))
        .map(|(inner, tail)| (inner, base.add_modifier(Modifier::ITALIC), None, tail))
}

fn push(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.content.to_mut().push_str(text);
        return;
    }
    spans.push(Span::styled(text.to_string(), style));
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/widgets/markdown/tests.rs"]
mod tests;
