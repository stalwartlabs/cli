/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::fmt;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Named(NamedColor),
    Rgb(u8, u8, u8),
    Default,
}

impl Color {
    pub fn parse(raw: &str) -> Self {
        let named = hashify::map_ignore_case!(
            raw.as_bytes(),
            NamedColor,
            "black" => NamedColor::Black,
            "red" => NamedColor::Red,
            "green" => NamedColor::Green,
            "yellow" => NamedColor::Yellow,
            "blue" => NamedColor::Blue,
            "magenta" => NamedColor::Magenta,
            "cyan" => NamedColor::Cyan,
            "white" => NamedColor::White,
        );
        match named {
            Some(color) => Color::Named(*color),
            None => Self::parse_hex(raw).unwrap_or(Color::Default),
        }
    }

    fn parse_hex(raw: &str) -> Option<Self> {
        let digits = raw.trim().strip_prefix('#')?.as_bytes();
        match *digits {
            [r1, r0, g1, g0, b1, b0] => Some(Color::Rgb(
                Self::hex_pair(r1, r0)?,
                Self::hex_pair(g1, g0)?,
                Self::hex_pair(b1, b0)?,
            )),
            [r, g, b] => Some(Color::Rgb(
                Self::hex_digit(r)? * 17,
                Self::hex_digit(g)? * 17,
                Self::hex_digit(b)? * 17,
            )),
            _ => None,
        }
    }

    fn hex_pair(high: u8, low: u8) -> Option<u8> {
        Some((Self::hex_digit(high)? << 4) | Self::hex_digit(low)?)
    }

    fn hex_digit(digit: u8) -> Option<u8> {
        match digit {
            b'0'..=b'9' => Some(digit - b'0'),
            b'a'..=b'f' => Some(digit - b'a' + 10),
            b'A'..=b'F' => Some(digit - b'A' + 10),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Plain,
    Bold,
    Dim,
    Accent,
    Success,
    Failure,
    ErrorTag,
    WarningTag,
    Heading,
    Badge(Color),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    end: usize,
    style: Style,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Styled {
    text: String,
    runs: Vec<Run>,
}

impl Styled {
    pub fn push(&mut self, text: &str, style: Style) {
        self.text.push_str(text);
        self.close_run(style);
    }

    pub fn plain(&mut self, text: &str) {
        self.push(text, Style::Plain);
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn into_text(self) -> String {
        self.text
    }

    pub fn width(&self) -> usize {
        self.text.width()
    }

    pub fn tidy(&mut self, max: usize) {
        let source = std::mem::take(self);
        self.text.reserve(source.text.len());
        let mut visible = 0usize;
        let mut prev_space = false;
        'runs: for (chunk, style) in source.runs() {
            for c in chunk.chars() {
                if matches!(c, '\n' | '\r' | '\t') {
                    if !prev_space {
                        self.text.push(' ');
                        visible += 1;
                        prev_space = true;
                    }
                } else {
                    self.text.push(c);
                    visible += c.width().unwrap_or(0);
                    prev_space = false;
                }
                if visible >= max {
                    self.close_run(style);
                    self.plain("…");
                    break 'runs;
                }
            }
            self.close_run(style);
        }
    }

    fn close_run(&mut self, style: Style) {
        let end = self.text.len();
        match self.runs.last_mut() {
            Some(run) if run.style == style => run.end = end,
            _ => self.runs.push(Run { end, style }),
        }
    }

    pub fn runs(&self) -> impl Iterator<Item = (&str, Style)> {
        let mut start = 0;
        self.runs.iter().map(move |run| {
            let text = self.text.get(start..run.end).unwrap_or_default();
            start = run.end;
            (text, run.style)
        })
    }
}

impl fmt::Write for Styled {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.plain(s);
        Ok(())
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/view/styled/tests.rs"]
mod tests;
