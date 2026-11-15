/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use super::{LiveStatus, Load, Popup, State, Zone, format_window, grid};
use crate::schema::MetricFormat;
use crate::schema::{CardSource, DashboardItem, ItemKind, Tone};
use crate::tui::app::ViewCx;
use crate::tui::chart::{self, ChartSpec, RankRow, SeriesData};
use crate::tui::theme::Theme;
use crate::view::metrics::format::format_value;
use crate::view::metrics::{Direction, ItemData, RangePreset, Sentiment, Window};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Gauge, Paragraph, Wrap};
use std::fmt::Write;

const MS_PER_SECOND: f64 = 1000.0;
const NO_DATA: &str = "No data";
const MIN_VISIBLE: u16 = 3;
const BORDER_WIDTH: usize = 2;
const DETAILS_HEIGHT: u16 = 4;
const TAB_LOOKBEHIND: usize = 4;

#[derive(Default)]
pub struct Buffers {
    spark: Vec<u64>,
    x: Vec<f64>,
    series: Vec<Vec<f64>>,
    labels: Vec<String>,
    text: String,
}

struct Panel<'a> {
    item: &'a DashboardItem,
    data: Option<&'a ItemData>,
    load: &'a Load,
    stream: &'a LiveStatus,
    focused: bool,
    expanded: bool,
}

pub fn render(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    render_body(state, frame, area, cx);
    let theme = cx.theme;
    match &mut state.popup {
        Popup::None => {}
        Popup::Nodes(picker) => picker.render(frame, area, theme),
        Popup::Range(input) => {
            let [row] = Layout::vertical([Constraint::Length(1)]).areas(area);
            frame.render_widget(Clear, row);
            let [label, field] = Layout::horizontal([
                Constraint::Length(RANGE_LABEL.len() as u16),
                Constraint::Fill(1),
            ])
            .areas(row);
            frame.render_widget(Line::styled(RANGE_LABEL, theme.strong()), label);
            frame.render_widget(input.line(theme, field.width, cx.focused), field);
        }
    }
}

const RANGE_LABEL: &str = " from to (UTC, YYYY-MM-DD[THH:MM]) ";

fn render_body(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    let [tabs, body] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    render_tabs(state, frame, tabs, cx);
    let body = match &state.stream {
        LiveStatus::Failed(error) => {
            let [banner, rest] =
                Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(body);
            let style = Style::new().fg(theme.err);
            frame.render_widget(
                Line::from(vec![
                    Span::styled(
                        " ✗ live updates stopped: ",
                        style.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(&**error, style),
                ]),
                banner,
            );
            rest
        }
        LiveStatus::Idle | LiveStatus::Connecting | LiveStatus::Open | LiveStatus::Reconnecting => {
            body
        }
    };
    let schema = state.schema.clone();
    let Some(dashboard) = schema.dashboards.get(state.tab) else {
        frame.render_widget(
            Paragraph::new(Line::styled(
                " No dashboards on this server.",
                theme.muted(),
            )),
            body,
        );
        return;
    };
    let items = dashboard.items();
    let data = match &state.data {
        Load::Ready(data) => Some(data),
        _ => None,
    };
    let item_data = |i: usize| data.and_then(|d| d.items.get(i));
    if state.expanded
        && let Some(item) = items.get(state.focus)
    {
        let [main, info] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(DETAILS_HEIGHT)]).areas(body);
        let panel = Panel {
            item,
            data: item_data(state.focus),
            load: &state.data,
            stream: &state.stream,
            focused: true,
            expanded: true,
        };
        draw(
            frame,
            main,
            &panel,
            theme,
            &state.window,
            &mut state.buffers,
        );
        render_details(
            frame,
            info,
            item,
            state.custom.is_none().then_some(state.range),
            &state.window,
            theme,
        );
        return;
    }
    let total = grid::layout(&state.placed, body.width, body.height, &mut state.rects);
    if let Some(rect) = state.rects.get(state.focus) {
        if rect.y < state.scroll {
            state.scroll = rect.y;
        } else if rect.bottom() > state.scroll.saturating_add(body.height) {
            state.scroll = rect.bottom().saturating_sub(body.height).min(rect.y);
        }
    }
    state.scroll = state.scroll.min(total.saturating_sub(body.height));
    for (i, (item, rect)) in items.iter().zip(&state.rects).enumerate() {
        let shifted = Rect {
            x: body.x + rect.x,
            y: body.y.saturating_add(rect.y).saturating_sub(state.scroll),
            ..*rect
        };
        if rect.bottom() <= state.scroll {
            continue;
        }
        let visible = shifted.intersection(body);
        if visible.height < MIN_VISIBLE {
            continue;
        }
        let panel = Panel {
            item,
            data: item_data(i),
            load: &state.data,
            stream: &state.stream,
            focused: i == state.focus && cx.focused && state.zone == Zone::Panels,
            expanded: false,
        };
        draw(
            frame,
            visible,
            &panel,
            theme,
            &state.window,
            &mut state.buffers,
        );
    }
}

fn render_tabs(state: &State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    let focused = cx.focused && state.zone == Zone::Tabs;
    let mut right = vec![Span::styled(
        format!(
            "{}/{}  range ",
            state.tab + 1,
            state.schema.dashboards.len()
        ),
        theme.muted(),
    )];
    for preset in RangePreset::ALL {
        let style = if state.custom.is_none() && preset == state.range {
            theme.selected
        } else {
            theme.muted()
        };
        right.push(Span::styled(
            [" ", preset.short_label(), " "].concat(),
            style,
        ));
    }
    if let Some(window) = state.custom {
        let mut text = String::from(" ");
        let _ = format_window(&window, " to ", &mut text);
        text.push(' ');
        right.push(Span::styled(text, theme.selected));
    }
    if let Some(node) = state.node {
        right.push(Span::styled(format!("  node {node}"), theme.muted()));
    }
    if state.frozen {
        right.push(Span::styled("  frozen", theme.muted()));
    }
    right.push(Span::raw(" "));
    let right = Line::from(right);
    let budget = usize::from(area.width).saturating_sub(right.width() + 4);
    let dashboards = &state.schema.dashboards;
    let start = state.tab.saturating_sub(TAB_LOOKBEHIND);
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    if start > 0 {
        spans.push(Span::styled(format!("‹{start} "), theme.muted()));
        used += 4;
    }
    let border = Style::new().fg(theme.border);
    for (i, dashboard) in dashboards.iter().enumerate().skip(start) {
        let width = dashboard.label.chars().count() + 3;
        if used + width + 4 > budget {
            spans.push(Span::styled(
                format!(" {}›", dashboards.len() - i),
                theme.muted(),
            ));
            break;
        }
        used += width;
        let style = if i == state.tab && focused {
            theme.selected
        } else if i == state.tab {
            Style::new()
                .fg(theme.brand)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            theme.muted()
        };
        spans.push(Span::styled([" ", &dashboard.label, " "].concat(), style));
        spans.push(Span::styled("│", border));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    frame.render_widget(Paragraph::new(right.right_aligned()), area);
}

fn title(kind: &ItemKind) -> &str {
    match kind {
        ItemKind::Stat(card) => &card.title,
        ItemKind::Chart(chart) => &chart.title,
        ItemKind::Headline(headline) => &headline.title,
        ItemKind::Ranking(ranking) => &ranking.title,
        ItemKind::Unknown => "Unsupported panel",
    }
}

fn description(kind: &ItemKind) -> Option<&str> {
    match kind {
        ItemKind::Stat(card) => card.description.as_deref(),
        ItemKind::Chart(chart) => chart.description.as_deref(),
        ItemKind::Headline(headline) => headline.description.as_deref(),
        ItemKind::Ranking(ranking) => ranking.description.as_deref(),
        ItemKind::Unknown => None,
    }
}

fn draw(
    frame: &mut Frame,
    area: Rect,
    panel: &Panel<'_>,
    theme: &Theme,
    window: &Window,
    buffers: &mut Buffers,
) {
    let kind = &panel.item.kind;
    let mut heading = vec![Span::raw(" "), Span::raw(title(kind)), Span::raw(" ")];
    if let ItemKind::Stat(card) = kind
        && card.source == CardSource::Live
    {
        let (chip, color) = match panel.stream {
            LiveStatus::Open => ("● live ", theme.ok),
            LiveStatus::Idle | LiveStatus::Connecting => ("○ live ", theme.dim),
            LiveStatus::Reconnecting => ("● reconnecting ", theme.warn),
            LiveStatus::Failed(_) => ("✗ live ", theme.err),
        };
        let room = usize::from(area.width).saturating_sub(BORDER_WIDTH);
        let chip = if title(kind).chars().count() + 2 + chip.chars().count() > room {
            chip.split_once(' ').map_or(chip, |(mark, _)| mark)
        } else {
            chip
        };
        heading.push(Span::styled(chip, Style::new().fg(color)));
    }
    let mut block = theme.panel(Line::from(heading), panel.focused);
    if panel.focused && !panel.expanded {
        block = block.title_bottom(Line::styled(" e expand ", theme.muted()).right_aligned());
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let inner = inner.inner(Margin::new(1, 0));
    let Some(data) = panel.data else {
        let text = match panel.load {
            Load::Failed(error) => Line::styled(&**error, Style::new().fg(theme.err)),
            Load::Waiting | Load::Ready(_) => Line::styled("no data yet", theme.muted()),
        };
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);
        return;
    };
    let text = &mut buffers.text;
    text.clear();
    match (kind, data) {
        (ItemKind::Stat(card), ItemData::Stat(stat)) => {
            format_value(stat.value, card.format, text);
            let tone = match panel.item.placement.tone {
                Some(Tone::Accent) => theme.brand,
                Some(Tone::Ink) => theme.accent,
                Some(Tone::Flat | Tone::Unknown) | None => theme.fg,
            };
            let mut line = vec![Span::styled(
                text.as_str(),
                Style::new().fg(tone).add_modifier(Modifier::BOLD),
            )];
            if card.delta
                && let Some(delta) = stat.delta
            {
                let arrow = match delta.direction {
                    Direction::Up => "▲",
                    Direction::Down => "▼",
                    Direction::Neutral => "■",
                };
                let color = match delta.sentiment(card.polarity) {
                    Sentiment::Good => theme.ok,
                    Sentiment::Bad => theme.err,
                    Sentiment::Neutral => theme.dim,
                };
                line.push(Span::styled(
                    format!("  {arrow} {:.1}%", delta.pct.abs()),
                    Style::new().fg(color),
                ));
                let mut prior = String::from(" vs prior ");
                let _ = window.write_half(&mut prior);
                let used = line.iter().map(Span::width).sum::<usize>() + prior.chars().count();
                if used <= usize::from(inner.width) {
                    line.push(Span::styled(prior, theme.muted()));
                }
            }
            let [value_area, rest] =
                Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
            frame.render_widget(Paragraph::new(Line::from(line)), value_area);
            match stat.sparkline.as_deref() {
                Some(values) if card.sparkline => {
                    chart::sparkline(frame, rest, values, theme.series[1], &mut buffers.spark)
                }
                _ => {
                    if let Some(description) = card.description.as_deref() {
                        frame.render_widget(
                            Paragraph::new(Line::styled(description, theme.muted()))
                                .wrap(Wrap { trim: true }),
                            rest,
                        );
                    }
                }
            }
        }
        (ItemKind::Headline(_), ItemData::Headline { ratio }) => {
            let [label, gauge] =
                Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);
            let Some(ratio) = ratio.filter(|ratio| ratio.is_finite()) else {
                frame.render_widget(Paragraph::new(Line::styled(NO_DATA, theme.muted())), label);
                return;
            };
            format_value(ratio * 100.0, MetricFormat::Percent, text);
            frame.render_widget(
                Paragraph::new(Line::styled(text.as_str(), theme.title())),
                label,
            );
            frame.render_widget(
                Gauge::default()
                    .ratio(ratio.clamp(0.0, 1.0))
                    .label("")
                    .gauge_style(Style::new().fg(theme.ok).bg(theme.border))
                    .use_unicode(true),
                gauge,
            );
        }
        (ItemKind::Chart(spec), ItemData::Chart(series)) => {
            buffers.x.clear();
            buffers.x.extend(
                (0..usize::from(window.buckets))
                    .map(|i| window.bucket_center(i).millis() as f64 / MS_PER_SECOND),
            );
            if buffers.series.len() < series.series.len() {
                buffers.series.resize_with(series.series.len(), Vec::new);
            }
            let format = spec.value_format.unwrap_or_default();
            let scale = chart::display_scale(format);
            for (out, values) in buffers.series.iter_mut().zip(&series.series) {
                out.clear();
                out.extend(values.iter().map(|v| v.map_or(f64::NAN, |v| v * scale)));
            }
            let data: Vec<SeriesData<'_>> = spec
                .series
                .iter()
                .zip(&buffers.series)
                .zip(theme.series.iter().cycle())
                .map(|((s, values), &color)| SeriesData {
                    label: &s.label,
                    values,
                    color,
                })
                .collect();
            chart::render(
                frame,
                inner,
                &ChartSpec {
                    kind: spec.kind,
                    stacked: spec.stacked,
                    x: &buffers.x,
                    series: &data,
                    format,
                },
            );
        }
        (ItemKind::Ranking(spec), ItemData::Ranking(entries)) => {
            let format = spec.value_format.unwrap_or_default();
            buffers.labels.truncate(entries.len());
            buffers.labels.resize_with(entries.len(), String::new);
            for (out, entry) in buffers.labels.iter_mut().zip(entries) {
                out.clear();
                format_value(entry.value, format, out);
            }
            let rows: Vec<RankRow<'_>> = entries
                .iter()
                .zip(&buffers.labels)
                .map(|(entry, text)| RankRow {
                    label: spec.series.get(entry.series).map_or("", |s| &s.label),
                    value: entry.value,
                    text,
                })
                .collect();
            chart::ranking(frame, inner, theme, &rows);
        }
        _ => frame.render_widget(
            Paragraph::new(Line::styled(
                "this panel type is not supported",
                theme.muted(),
            )),
            inner,
        ),
    }
}

fn render_details(
    frame: &mut Frame,
    area: Rect,
    item: &DashboardItem,
    range: Option<RangePreset>,
    window: &Window,
    theme: &Theme,
) {
    let block = theme.panel(" Details ", false).title_bottom(
        Line::styled(" e/esc restore · t range · f freeze ", theme.muted()).right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut info = String::new();
    let _ = write!(info, " · {} buckets", window.buckets);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                [" ", description(&item.kind).unwrap_or("No description.")].concat(),
                theme.text(),
            ),
            Line::from(vec![
                Span::styled(" range ", theme.muted()),
                Span::styled(range.map_or("custom", RangePreset::label), theme.title()),
                Span::styled(info, theme.muted()),
            ]),
        ])
        .wrap(Wrap { trim: false }),
        inner,
    );
}
