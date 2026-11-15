/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::view::{Color as ViewColor, NamedColor, Style as ViewStyle, Styled};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Palette {
    Dark,
    Light,
    Ansi16,
    Mono,
}

impl Palette {
    pub fn next(self) -> Self {
        match self {
            Palette::Dark => Palette::Light,
            Palette::Light => Palette::Ansi16,
            Palette::Ansi16 => Palette::Mono,
            Palette::Mono => Palette::Dark,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Palette::Dark => "dark",
            Palette::Light => "light",
            Palette::Ansi16 => "ansi16",
            Palette::Mono => "mono",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Palette::Ansi16 => "16 colours",
            palette => palette.name(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteRequest {
    Forced(Palette),
    Default(Palette),
}

impl PaletteRequest {
    pub fn resolve(self, remembered: Option<Palette>) -> Palette {
        match self {
            PaletteRequest::Forced(palette) => palette,
            PaletteRequest::Default(palette) => remembered.unwrap_or(palette),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Ok,
    Warn,
    Error,
}

pub struct Theme {
    pub palette: Palette,
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub surface: Color,
    pub brand: Color,
    pub brand_fg: Color,
    pub accent: Color,
    pub border: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    pub info: Color,
    pub input_bg: Color,
    pub series: [Color; 4],
    pub selected: Style,
    pub key: Style,
    pub pill: Style,
}

const fn hex(rgb: u32) -> Color {
    Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

const BOLD: Modifier = Modifier::BOLD;
const REVERSED_BOLD: Modifier = Modifier::REVERSED.union(Modifier::BOLD);

impl Theme {
    pub fn new(palette: Palette) -> Self {
        match palette {
            Palette::Dark => Self {
                palette,
                bg: Color::Reset,
                fg: hex(0xf2f2f3),
                dim: hex(0x9e9ea4),
                surface: hex(0x232326),
                brand: hex(0xdb2d54),
                brand_fg: Color::White,
                accent: hex(0xff8199),
                border: hex(0x3a3a40),
                ok: hex(0x6fcf97),
                warn: hex(0xf2c46d),
                err: hex(0xe0602f),
                info: hex(0x7fb3ec),
                input_bg: hex(0x2a2a2e),
                series: [hex(0xff5c7f), hex(0x4cc3b3), hex(0xf0a35a), hex(0xa99bf0)],
                selected: Style::new()
                    .bg(hex(0x45212d))
                    .fg(hex(0xf2f2f3))
                    .add_modifier(BOLD),
                key: Style::new().fg(hex(0xc4b5a5)).add_modifier(BOLD),
                pill: Style::new()
                    .bg(hex(0x33291f))
                    .fg(hex(0xecdcc8))
                    .add_modifier(BOLD),
            },
            Palette::Light => Self {
                palette,
                bg: hex(0xf5f2ec),
                fg: hex(0x1c1a17),
                dim: hex(0x6b645b),
                surface: hex(0xfffdf9),
                brand: hex(0xdb2d54),
                brand_fg: Color::White,
                accent: hex(0xbf1f45),
                border: hex(0xd6d0c6),
                ok: hex(0x2f7d4f),
                warn: hex(0xa16207),
                err: hex(0xc2410c),
                info: hex(0x2563a8),
                input_bg: hex(0xefe9df),
                series: [hex(0xdb2d54), hex(0x2a9d8f), hex(0xd9822b), hex(0x4a3f8f)],
                selected: Style::new()
                    .bg(hex(0xf7d5db))
                    .fg(hex(0x1c1a17))
                    .add_modifier(BOLD),
                key: Style::new().fg(hex(0x6e5d4b)).add_modifier(BOLD),
                pill: Style::new()
                    .bg(hex(0xecdcc8))
                    .fg(hex(0x1c1a17))
                    .add_modifier(BOLD),
            },
            Palette::Ansi16 => Self {
                palette,
                bg: Color::Reset,
                fg: Color::Reset,
                dim: Color::DarkGray,
                surface: Color::Reset,
                brand: Color::Red,
                brand_fg: Color::White,
                accent: Color::LightRed,
                border: Color::DarkGray,
                ok: Color::Green,
                warn: Color::Yellow,
                err: Color::Red,
                info: Color::Blue,
                input_bg: Color::Reset,
                series: [Color::LightRed, Color::Cyan, Color::Yellow, Color::Magenta],
                selected: Style::new().add_modifier(REVERSED_BOLD),
                key: Style::new().fg(Color::Yellow).add_modifier(BOLD),
                pill: Style::new().add_modifier(REVERSED_BOLD),
            },
            Palette::Mono => Self {
                palette,
                bg: Color::Reset,
                fg: Color::Reset,
                dim: Color::Reset,
                surface: Color::Reset,
                brand: Color::Reset,
                brand_fg: Color::Reset,
                accent: Color::Reset,
                border: Color::Reset,
                ok: Color::Reset,
                warn: Color::Reset,
                err: Color::Reset,
                info: Color::Reset,
                input_bg: Color::Reset,
                series: [Color::Reset; 4],
                selected: Style::new().add_modifier(Modifier::REVERSED),
                key: Style::new().add_modifier(BOLD),
                pill: Style::new().add_modifier(REVERSED_BOLD),
            },
        }
    }

    pub fn text(&self) -> Style {
        Style::new().fg(self.fg)
    }

    pub fn muted(&self) -> Style {
        Style::new().fg(self.dim)
    }

    pub fn title(&self) -> Style {
        Style::new().fg(self.fg).add_modifier(BOLD)
    }

    pub fn strong(&self) -> Style {
        Style::new().fg(self.brand).add_modifier(BOLD)
    }

    pub fn level(&self, level: Level) -> Color {
        match level {
            Level::Info => self.info,
            Level::Ok => self.ok,
            Level::Warn => self.warn,
            Level::Error => self.err,
        }
    }

    pub fn border(&self, focused: bool) -> Style {
        if focused {
            Style::new().fg(self.brand)
        } else {
            Style::new().fg(self.border)
        }
    }

    pub fn popup(&self) -> Style {
        Style::new().bg(self.surface).fg(self.fg)
    }

    pub fn base(&self) -> Style {
        Style::new().bg(self.bg).fg(self.fg)
    }

    pub fn input(&self) -> Style {
        match self.palette {
            Palette::Dark | Palette::Light => Style::new().bg(self.input_bg).fg(self.fg),
            Palette::Ansi16 | Palette::Mono => Style::new().add_modifier(Modifier::UNDERLINED),
        }
    }

    pub fn panel<'a>(&self, title: impl Into<Line<'a>>, focused: bool) -> Block<'a> {
        let title: Line<'a> = title.into();
        let style = if focused { self.strong() } else { self.title() };
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(self.border(focused))
            .title(title.style(style))
    }

    pub fn popup_block<'a>(&self, title: impl Into<Line<'a>>, accent: Color) -> Block<'a> {
        let title: Line<'a> = title.into();
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(accent))
            .title(title.style(Style::new().fg(accent).add_modifier(BOLD)))
            .style(self.popup())
    }

    pub fn color(&self, color: ViewColor) -> Color {
        match (self.palette, color) {
            (Palette::Mono, _) | (_, ViewColor::Default) => Color::Reset,
            (Palette::Dark | Palette::Light, ViewColor::Rgb(r, g, b)) => Color::Rgb(r, g, b),
            (Palette::Ansi16, ViewColor::Rgb(r, g, b)) => nearest_ansi(r, g, b),
            (_, ViewColor::Named(named)) => self.named(named),
        }
    }

    fn named(&self, named: NamedColor) -> Color {
        match named {
            NamedColor::Red => self.err,
            NamedColor::Green => self.ok,
            NamedColor::Yellow => self.warn,
            NamedColor::Blue | NamedColor::Cyan => self.info,
            NamedColor::Magenta => self.accent,
            NamedColor::Black | NamedColor::White => self.fg,
        }
    }

    pub fn style(&self, style: ViewStyle) -> Style {
        match style {
            ViewStyle::Plain => self.text(),
            ViewStyle::Dim => self.muted(),
            ViewStyle::Bold | ViewStyle::Heading => self.title(),
            ViewStyle::Accent => Style::new().fg(self.accent),
            ViewStyle::Success => Style::new().fg(self.ok),
            ViewStyle::Failure => Style::new().fg(self.err),
            ViewStyle::ErrorTag => Style::new().fg(self.err).add_modifier(BOLD),
            ViewStyle::WarningTag => Style::new().fg(self.warn).add_modifier(BOLD),
            ViewStyle::Badge(color) => Style::new().fg(self.color(color)).add_modifier(BOLD),
        }
    }

    pub fn spans<'a>(&self, styled: &'a Styled) -> impl Iterator<Item = Span<'a>> + use<'a, '_> {
        styled
            .runs()
            .map(|(text, style)| Span::styled(text, self.style(style)))
    }
}

fn nearest_ansi(r: u8, g: u8, b: u8) -> Color {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max.abs_diff(min) < 32 {
        return if max > 160 {
            Color::Gray
        } else {
            Color::DarkGray
        };
    }
    let bright = max > 200;
    let hot = |c: u8| c > max / 2 + 16;
    match (hot(r), hot(g), hot(b), bright) {
        (true, false, false, false) => Color::Red,
        (true, false, false, true) => Color::LightRed,
        (false, true, false, false) => Color::Green,
        (false, true, false, true) => Color::LightGreen,
        (false, false, true, false) => Color::Blue,
        (false, false, true, true) => Color::LightBlue,
        (true, true, false, false) => Color::Yellow,
        (true, true, false, true) => Color::LightYellow,
        (true, false, true, false) => Color::Magenta,
        (true, false, true, true) => Color::LightMagenta,
        (false, true, true, false) => Color::Cyan,
        (false, true, true, true) => Color::LightCyan,
        _ => Color::Gray,
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/tui/theme/tests.rs"]
mod tests;
