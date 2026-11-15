/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::{ChartKind, MetricFormat};
use crate::tui::theme::Theme;
use crate::tui::widgets::chrome::pad;
use malevich::scale::Unit;
use malevich::{Area, Bars, Charset, Line, Plot};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Bar, BarChart, Sparkline};

const CHARSET: Charset = Charset::Quadrants;
const MS_PER_SECOND: f64 = 1000.0;
const SPARK_SCALE: f64 = 1000.0;
const BAR_FILL: f64 = 0.8;

pub struct SeriesData<'a> {
    pub label: &'a str,
    pub values: &'a [f64],
    pub color: Color,
}

pub struct ChartSpec<'a> {
    pub kind: ChartKind,
    pub stacked: bool,
    pub x: &'a [f64],
    pub series: &'a [SeriesData<'a>],
    pub format: MetricFormat,
}

pub struct RankRow<'a> {
    pub label: &'a str,
    pub value: f64,
    pub text: &'a str,
}

pub fn render(frame: &mut Frame, area: Rect, spec: &ChartSpec<'_>) {
    if area.is_empty() || spec.x.is_empty() || spec.series.is_empty() {
        return;
    }
    let bands = match (spec.kind, spec.stacked) {
        (ChartKind::Bar, _) | (_, true) => {
            let refs: Vec<&[f64]> = spec.series.iter().map(|s| s.values).collect();
            malevich::stat::stack(&refs)
        }
        _ => Vec::new(),
    };
    let step = bucket_step(spec.x);
    let mut plot = Plot::new().time_x().y_unit(unit(spec.format)).y_min(0.0);
    for (i, series) in spec.series.iter().enumerate() {
        let y = series.values;
        let color = color(series.color);
        plot = match (spec.kind, bands.get(i)) {
            (ChartKind::Bar, Some((low, high))) => plot.layer(
                Bars::at(spec.x, step * BAR_FILL, &high[..])
                    .base(&low[..])
                    .label(series.label)
                    .color(color),
            ),
            (_, Some((low, high))) => plot.layer(
                Area::between(spec.x, &low[..], &high[..])
                    .label(series.label)
                    .color(color),
            ),
            (ChartKind::Area, None) => {
                plot.layer(Area::xy(spec.x, y).label(series.label).color(color))
            }
            (_, None) => plot.layer(Line::xy(spec.x, y).label(series.label).color(color)),
        };
    }
    frame.render_widget(plot.widget().charset(CHARSET), area);
}

pub fn sparkline(frame: &mut Frame, area: Rect, values: &[f64], color: Color, buf: &mut Vec<u64>) {
    let width = usize::from(area.width);
    if width == 0 || values.is_empty() {
        return;
    }
    resample(values, width, buf);
    frame.render_widget(
        Sparkline::default()
            .data(buf.as_slice())
            .style(Style::new().fg(color)),
        area,
    );
}

pub fn ranking(frame: &mut Frame, area: Rect, theme: &Theme, rows: &[RankRow<'_>]) {
    let label_width = rows
        .iter()
        .map(|r| r.label.chars().count())
        .max()
        .unwrap_or(0);
    let max = rows.iter().fold(0.0f64, |m, r| m.max(r.value));
    let bars: Vec<Bar<'_>> = rows
        .iter()
        .zip(theme.series.iter().cycle())
        .map(|(row, &color)| {
            let gap = label_width.saturating_sub(row.label.chars().count()) + 1;
            let label = TextLine::from(vec![
                Span::raw(row.label),
                Span::raw(pad(gap)),
                Span::raw(row.text),
            ]);
            Bar::default()
                .value(spark_value(row.value, max))
                .label(label)
                .text_value(String::new())
                .style(Style::new().fg(color))
                .value_style(Style::new().fg(theme.fg).add_modifier(Modifier::BOLD))
        })
        .collect();
    frame.render_widget(
        BarChart::horizontal(bars)
            .bar_width(1)
            .bar_gap(0)
            .max(SPARK_SCALE as u64)
            .label_style(theme.muted()),
        area,
    );
}

fn resample(values: &[f64], width: usize, buf: &mut Vec<u64>) {
    buf.clear();
    let max = values.iter().fold(0.0f64, |m, &v| m.max(v));
    let len = values.len();
    buf.extend((0..width).map(|i| {
        let start = i * len / width;
        let end = ((i + 1) * len / width).max(start + 1).min(len);
        let peak = values
            .get(start..end)
            .map_or(0.0, |s| s.iter().fold(0.0f64, |m, &v| m.max(v)));
        spark_value(peak, max)
    }));
}

fn spark_value(value: f64, max: f64) -> u64 {
    if max > 0.0 && value.is_finite() && value > 0.0 {
        (value / max * SPARK_SCALE).round().clamp(0.0, SPARK_SCALE) as u64
    } else {
        0
    }
}

fn bucket_step(x: &[f64]) -> f64 {
    match (x.first(), x.last()) {
        (Some(first), Some(last)) if x.len() > 1 => (last - first) / (x.len() - 1) as f64,
        _ => 1.0,
    }
}

pub fn display_scale(format: MetricFormat) -> f64 {
    match format {
        MetricFormat::Duration => MS_PER_SECOND.recip(),
        _ => 1.0,
    }
}

fn unit(format: MetricFormat) -> Unit {
    match format {
        MetricFormat::Bytes => Unit::Bytes,
        MetricFormat::Duration => Unit::si("s"),
        MetricFormat::Percent => Unit::suffix("%"),
        MetricFormat::Number | MetricFormat::Unknown => Unit::Plain,
    }
}

fn color(color: Color) -> malevich::Color {
    use malevich::Color as M;
    match color {
        Color::Rgb(r, g, b) => M::Rgb(r, g, b),
        Color::Indexed(i) => M::Ansi256(i),
        Color::Black => M::Black,
        Color::Red => M::Red,
        Color::Green => M::Green,
        Color::Yellow => M::Yellow,
        Color::Blue => M::Blue,
        Color::Magenta => M::Magenta,
        Color::Cyan => M::Cyan,
        Color::Gray => M::White,
        Color::DarkGray => M::BrightBlack,
        Color::LightRed => M::BrightRed,
        Color::LightGreen => M::BrightGreen,
        Color::LightYellow => M::BrightYellow,
        Color::LightBlue => M::BrightBlue,
        Color::LightMagenta => M::BrightMagenta,
        Color::LightCyan => M::BrightCyan,
        Color::White => M::BrightWhite,
        Color::Reset => M::Default,
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/chart/tests.rs"]
mod tests;
