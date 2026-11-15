/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::{FieldType, Schema};
use crate::tui::form::editor::Kind;
use crate::tui::form::expression;
use crate::tui::form::map::{MapEditing, MapView};
use crate::tui::form::model::{
    Access, CHANGED_SECRET, Marker, PASSKEY_REGISTRATION, RowKind, is_changed_secret, is_missing,
    item_summary,
};
use crate::tui::form::nested::{Frame as NestedFrame, ListView};
use crate::tui::form::otp::{self, OTP_AUTH};
use crate::tui::form::view::FormView;
use crate::tui::theme::Theme;
use crate::tui::widgets::chrome::{self, pad};
use crate::tui::widgets::markdown;
use crate::view::{LabelCache, Styled, ValueFormatter};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

const HELP_WIDTH: u16 = 32;
const WIDE: u16 = 110;
const LABEL_WIDTH: usize = 24;
pub const EDIT_HEIGHT: usize = 8;
const HELP_STRIP: u16 = 4;
const HELP_STRIP_MIN_HEIGHT: u16 = 14;
const OPTIONAL: &str = "  (optional)";
pub const PASSKEY_HINT: &str = "Register passkeys in the web interface (o opens it)";
const LIST_HINT: &str = " ⏎ edit  a add  d remove  J/K move  esc done ";
const MAP_HINT: &str = " ⏎ edit value  a add  d remove  esc done ";
const MAP_KEY_WIDTH: usize = 32;

pub struct FormFrame<'a> {
    pub title: &'a str,
    pub schema: &'a Schema,
    pub labels: &'a LabelCache,
    pub theme: &'a Theme,
    pub focused: bool,
}

pub fn render(view: &mut FormView, frame: &mut Frame, area: Rect, cx: &FormFrame<'_>) {
    if view.frames().is_empty() {
        render_view(view, frame, area, cx);
        return;
    }
    let mut title = String::from(cx.title);
    for nested in view.frames() {
        nested.crumb(&mut title);
    }
    let cx = &FormFrame {
        title: &title,
        ..*cx
    };
    match view.top_frame_mut() {
        Some(NestedFrame::Form { view, .. }) => render_view(view, frame, area, cx),
        Some(NestedFrame::List { list, .. }) => render_list(list, frame, area, cx),
        Some(NestedFrame::Map { map, .. }) => render_map(map, frame, area, cx),
        Some(NestedFrame::Expression { expression, .. }) => {
            expression::render::render(expression, frame, area, cx)
        }
        None => {}
    }
}

fn render_list(list: &ListView, frame: &mut Frame, area: Rect, cx: &FormFrame<'_>) {
    let theme = cx.theme;
    let mut title = vec![Span::raw(" "), Span::raw(cx.title), Span::raw(" ")];
    if list.dirty {
        title.push(Span::styled("● unsaved ", Style::new().fg(theme.warn)));
    }
    let block = theme
        .panel(Line::from(title), cx.focused)
        .title_bottom(Line::styled(LIST_HINT, theme.muted()).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = Vec::with_capacity(list.items.len() + 2);
    if let Some(status) = list.status {
        lines.push(Line::styled(
            format!(" {status}"),
            Style::new().fg(theme.err),
        ));
    }
    if list.items.is_empty() {
        lines.push(Line::styled(" No items · a to add one", theme.muted()));
    }
    for (index, item) in list.items.iter().enumerate() {
        let focused = index == list.cursor;
        let mut text = String::new();
        list.item_title(cx.schema, item, &mut text);
        let style = if focused {
            theme.selected
        } else {
            theme.text()
        };
        lines.push(Line::from(vec![
            Span::styled(
                if focused { "▌" } else { " " },
                Style::new().fg(theme.brand),
            ),
            Span::styled(format!("{:>3}. ", index + 1), theme.muted()),
            Span::styled(text, style),
        ]));
    }
    let height = usize::from(inner.height);
    let offset = (list.cursor + usize::from(list.status.is_some()) + 1).saturating_sub(height);
    let scroll = u16::try_from(offset).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

fn render_map(map: &mut MapView, frame: &mut Frame, area: Rect, cx: &FormFrame<'_>) {
    let theme = cx.theme;
    let mut title = vec![Span::raw(" "), Span::raw(cx.title), Span::raw(" ")];
    if map.dirty {
        title.push(Span::styled("● unsaved ", Style::new().fg(theme.warn)));
    }
    let block = theme
        .panel(Line::from(title), cx.focused)
        .title_bottom(Line::styled(MAP_HINT, theme.muted()).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let formatter = ValueFormatter::new(cx.schema, cx.labels);
    let keys: Vec<Styled> = map
        .entries
        .iter()
        .map(|(name, _)| {
            let mut styled = Styled::default();
            match &map.key_type {
                FieldType::String { .. } => styled.plain(name),
                typ => formatter.inline(&mut styled, &Value::String(name.clone()), typ),
            }
            styled.tidy(MAP_KEY_WIDTH);
            styled
        })
        .collect();
    let key_width = keys
        .iter()
        .map(Styled::width)
        .chain([map.key_label.width()])
        .max()
        .unwrap_or(0);
    let value_width = usize::from(inner.width).saturating_sub(key_width + 6);
    let cursor = map.cursor;
    let mut editor: Vec<Line<'static>> = Vec::new();
    let mut new_key = false;
    if let Some(editing) = map.editing_mut() {
        let (inline, is_key) = match editing {
            MapEditing::Key(inline) => (inline, true),
            MapEditing::Value(inline) => (inline, false),
        };
        new_key = is_key;
        let width = u16::try_from(value_width).unwrap_or(u16::MAX);
        editor = inline
            .lines(theme, width, EDIT_HEIGHT)
            .into_iter()
            .map(owned_line)
            .collect();
    }
    let mut lines = Vec::with_capacity(map.entries.len() + 4);
    if let Some(status) = &map.status {
        lines.push(Line::styled(
            format!(" {status}"),
            Style::new().fg(theme.err),
        ));
    }
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{:<key_width$}", map.key_label), theme.title()),
        Span::raw("  "),
        Span::styled(&*map.value_label, theme.title()),
    ]));
    if map.entries.is_empty() && !new_key {
        lines.push(Line::styled(" No entries · a to add one", theme.muted()));
    }
    let header = lines.len();
    for (index, ((_, value), key)) in map.entries.iter().zip(&keys).enumerate() {
        let focused = index == cursor && !new_key;
        let mut spans = vec![
            Span::styled(
                if focused { "▌" } else { " " },
                Style::new().fg(theme.brand),
            ),
            Span::raw(" "),
        ];
        spans.extend(
            theme
                .spans(key)
                .map(|s| Span::styled(s.content.into_owned(), s.style)),
        );
        spans.push(Span::raw(pad(key_width.saturating_sub(key.width()) + 2)));
        if focused && !editor.is_empty() {
            let mut rest = editor.drain(..);
            if let Some(first) = rest.next() {
                spans.extend(first.spans);
            }
            lines.push(Line::from(spans));
            for line in rest {
                let mut indent = vec![Span::raw(pad(key_width + 4))];
                indent.extend(line.spans);
                lines.push(Line::from(indent));
            }
            continue;
        }
        let mut styled = Styled::default();
        match (value, &map.value_type) {
            (Value::Object(item), FieldType::Object { object_name, .. }) => {
                styled.plain(item_summary(cx.schema, object_name, item).unwrap_or("{…}"));
            }
            (value, typ) => formatter.inline(&mut styled, value, typ),
        }
        styled.tidy(value_width);
        spans.extend(
            theme
                .spans(&styled)
                .map(|s| Span::styled(s.content.into_owned(), s.style)),
        );
        lines.push(Line::from(spans));
    }
    if new_key {
        let mut spans = vec![
            Span::styled("▌", Style::new().fg(theme.brand)),
            Span::raw(" "),
            Span::styled("new key ", theme.muted()),
        ];
        for line in editor.drain(..).take(1) {
            spans.extend(line.spans);
        }
        lines.push(Line::from(spans));
    }
    let focus = if new_key {
        lines.len().saturating_sub(1)
    } else {
        header + cursor
    };
    let height = usize::from(inner.height);
    let offset = (focus + 1).saturating_sub(height);
    let scroll = u16::try_from(offset).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

fn render_view(view: &mut FormView, frame: &mut Frame, area: Rect, cx: &FormFrame<'_>) {
    let wide = area.width >= WIDE;
    let [body, help] = if wide {
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(HELP_WIDTH)]).areas(area)
    } else if area.height >= HELP_STRIP_MIN_HEIGHT
        && view
            .current_field(cx.schema)
            .is_some_and(|field| !field.description.is_empty())
    {
        Layout::vertical([Constraint::Fill(1), Constraint::Length(HELP_STRIP)]).areas(area)
    } else {
        [area, Rect::default()]
    };
    if wide {
        render_help(view, frame, help, cx);
    } else if help.height > 0 {
        render_strip(view, frame, help, cx);
    }
    render_fields(view, frame, body, cx);
}

fn render_strip(view: &FormView, frame: &mut Frame, area: Rect, cx: &FormFrame<'_>) {
    let Some(field) = view.current_field(cx.schema) else {
        return;
    };
    let block = cx.theme.panel(" Field ", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = Vec::new();
    markdown::render(&field.description, cx.theme, &mut lines);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_fields(view: &mut FormView, frame: &mut Frame, area: Rect, cx: &FormFrame<'_>) {
    let theme = cx.theme;
    let dirty = view.model.dirty_count();
    let errors = view.model.error_count();
    let mut title = vec![Span::raw(" "), Span::raw(cx.title), Span::raw(" ")];
    if dirty > 0 {
        title.push(Span::styled(
            format!("● {dirty} unsaved "),
            Style::new().fg(theme.warn),
        ));
    }
    if errors > 0 {
        title.push(Span::styled(
            format!("✗ {errors} error{} ", if errors == 1 { "" } else { "s" }),
            Style::new().fg(theme.err),
        ));
    }
    let block = theme.panel(Line::from(title), cx.focused).title_bottom(
        Line::styled(" * required  ^s review & save ", theme.muted()).right_aligned(),
    );
    let inner = block.inner(area);
    let value_width = inner
        .width
        .saturating_sub(u16::try_from(LABEL_WIDTH + 4).unwrap_or(0));
    let formatter = ValueFormatter::new(cx.schema, cx.labels);
    let fields = view.model.fields(cx.schema);
    let mut lines: Vec<Line<'_>> = Vec::with_capacity(view.model.layout.rows.len() * 2);
    let mut focus_line = 0;
    let mut styled = Styled::default();
    let mut editor_lines = Vec::new();
    let editing = view.is_editing();
    if editing {
        editor_lines = view
            .inline_lines(theme, value_width, EDIT_HEIGHT)
            .into_iter()
            .map(owned_line)
            .collect();
    }
    let editor_height = editor_lines.len();
    if let Some(banner) = &view.model.banner {
        let room = usize::from(inner.width).saturating_sub(1).max(1);
        for chunk in banner
            .lines()
            .flat_map(|text| chrome::wrap_lines(text, room))
        {
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(chunk, Style::new().fg(theme.err)),
            ]));
        }
        lines.push(Line::raw(""));
    }
    let model = &view.model;
    let variant_label = model.at_type().map(|at_type| {
        model
            .shape
            .variants(cx.schema)
            .iter()
            .find(|variant| variant.name == at_type)
            .map_or(at_type, |variant| variant.label.as_str())
    });
    for section in &model.layout.sections {
        if let Some(title) = &section.title {
            let mark = if model.section_has_error(section) {
                Some(Span::styled(" ✗", Style::new().fg(theme.err)))
            } else if model.section_dirty(section) {
                Some(Span::styled(" ●", Style::new().fg(theme.warn)))
            } else {
                None
            };
            let marked = if mark.is_some() { 2 } else { 0 };
            let rule = usize::from(inner.width).saturating_sub(title.width() + marked + 6);
            let mut heading = Vec::with_capacity(5);
            heading.push(Span::styled("  ── ", theme.muted()));
            heading.push(Span::styled(&**title, theme.title()));
            heading.extend(mark);
            heading.push(Span::styled(" ", theme.muted()));
            heading.push(Span::styled("─".repeat(rule), theme.muted()));
            lines.push(Line::from(heading));
        }
        for (index, row) in model
            .layout
            .rows
            .iter()
            .enumerate()
            .skip(section.rows.start)
            .take(section.rows.len())
        {
            let focused = index == view.row;
            if focused {
                focus_line = lines.len();
            }
            let dirty = model.is_dirty(&row.name);
            let error = model.error(&row.name);
            let mut spans = Vec::with_capacity(8);
            spans.push(Span::styled(
                if focused { "▌" } else { " " },
                Style::new().fg(theme.brand),
            ));
            spans.push(if error.is_some() {
                Span::styled("✗", Style::new().fg(theme.err))
            } else if dirty {
                Span::styled("●", Style::new().fg(theme.warn))
            } else {
                Span::raw(" ")
            });
            let marker = match row.marker {
                Marker::Required => "*",
                Marker::Optional | Marker::Plain => " ",
            };
            let label_width = row.label.width() + 1;
            spans.push(Span::raw(pad(LABEL_WIDTH.saturating_sub(label_width))));
            let label_style = if focused { theme.title() } else { theme.text() };
            spans.push(Span::styled(&*row.label, label_style));
            spans.push(Span::styled(marker, Style::new().fg(theme.brand)));
            spans.push(Span::raw("  "));
            if focused && editing {
                let mut first = true;
                for line in editor_lines.drain(..) {
                    if first {
                        spans.extend(line.spans);
                        lines.push(Line::from(std::mem::take(&mut spans)));
                        first = false;
                    } else {
                        let mut indent = vec![Span::raw(pad(LABEL_WIDTH + 4))];
                        indent.extend(line.spans);
                        lines.push(Line::from(indent));
                    }
                }
                if first {
                    lines.push(Line::from(spans));
                }
            } else {
                let typ = fields
                    .and_then(|f| f.properties.get(&*row.name))
                    .map(|f| &f.typ);
                styled_value(
                    &mut styled,
                    &formatter,
                    model.value(&row.name),
                    typ,
                    row.kind,
                    variant_label,
                );
                styled.tidy(usize::from(value_width));
                spans.extend(
                    theme
                        .spans(&styled)
                        .map(|s| Span::styled(s.content.into_owned(), s.style)),
                );
                match row.access {
                    Access::Edit => {}
                    Access::ReadOnly => spans.push(Span::styled("  read-only", theme.muted())),
                    Access::Locked => {
                        spans.push(Span::styled("  🔒 enterprise", Style::new().fg(theme.warn)))
                    }
                }
                let placeholder = row
                    .placeholder
                    .as_deref()
                    .or_else(|| typ.and_then(Kind::of_field).and_then(Kind::placeholder));
                if let Some(placeholder) = placeholder
                    && is_missing(model.value(&row.name))
                {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(
                        placeholder,
                        theme.muted().add_modifier(Modifier::ITALIC),
                    ));
                }
                if row.marker == Marker::Optional {
                    spans.push(Span::styled(OPTIONAL, theme.muted()));
                }
                lines.push(Line::from(spans));
            }
            if let Some(error) = error {
                expression::render::error_lines(
                    &mut lines,
                    LABEL_WIDTH + 4,
                    error,
                    usize::from(inner.width),
                    Style::new().fg(theme.err),
                );
            }
        }
        lines.push(Line::raw(""));
    }
    let height = usize::from(inner.height);
    let margin = editor_height
        .min(height.saturating_sub(1))
        .max(EDIT_HEIGHT.min(height / 2));
    let top = if view.row == 0 { 0 } else { focus_line };
    if top < view.scroll {
        view.scroll = top;
    } else if focus_line + margin >= view.scroll + height {
        view.scroll = (focus_line + margin + 1).saturating_sub(height);
    }
    let block = if lines.len() > height {
        block.title_bottom(
            Line::styled(
                format!(" {} of {} ", view.row + 1, view.model.layout.rows.len()),
                theme.muted(),
            )
            .left_aligned(),
        )
    } else {
        block
    };
    frame.render_widget(block, area);
    let scroll = u16::try_from(view.scroll).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

fn styled_value(
    out: &mut Styled,
    formatter: &ValueFormatter<'_>,
    value: Option<&Value>,
    typ: Option<&FieldType>,
    kind: RowKind,
    variant: Option<&str>,
) {
    *out = Styled::default();
    match (kind, value, typ) {
        (RowKind::Variant, _, _) => out.plain(variant.unwrap_or("choose ▾")),
        (_, value, Some(FieldType::Object { object_name, .. })) if object_name == OTP_AUTH => {
            out.plain(otp::status(value))
        }
        (_, _, Some(FieldType::Object { object_name, .. }))
            if object_name == PASSKEY_REGISTRATION =>
        {
            out.push(PASSKEY_HINT, crate::view::Style::Dim)
        }
        (_, None, _) => out.push("not set", crate::view::Style::Dim),
        (_, Some(value), Some(typ)) if is_changed_secret(typ, value) => {
            out.push(CHANGED_SECRET, crate::view::Style::Dim)
        }
        (_, Some(value), Some(typ)) => formatter.inline(out, value, typ),
        (_, Some(Value::String(text)), None) => out.plain(text),
        (_, Some(value), None) => out.plain(&value.to_string()),
    }
}

pub fn owned_line(line: Line<'_>) -> Line<'static> {
    Line::from(
        line.spans
            .into_iter()
            .map(|span| Span::styled(span.content.into_owned(), span.style))
            .collect::<Vec<_>>(),
    )
}

fn render_help(view: &FormView, frame: &mut Frame, area: Rect, cx: &FormFrame<'_>) {
    let theme = cx.theme;
    let block = theme.panel(" Field ", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(row) = view.model.layout.rows.get(view.row) else {
        return;
    };
    let mut lines = vec![
        Line::styled(row.label.to_string(), theme.title()),
        Line::styled(row.name.to_string(), theme.muted()),
        Line::raw(""),
    ];
    if let Some(field) = view.current_field(cx.schema)
        && !field.description.is_empty()
    {
        markdown::render(&field.description, theme, &mut lines);
        lines.push(Line::raw(""));
    }
    if view.model.is_dirty(&row.name) {
        let before = view
            .model
            .original
            .get(&*row.name)
            .map_or_else(|| "not set".to_string(), Value::to_string);
        lines.push(Line::styled(
            format!("● was {before}"),
            Style::new().fg(theme.warn),
        ));
        lines.push(Line::styled("u reverts", theme.muted()));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/form/render/tests.rs"]
mod tests;
