/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::fmt::{self, Display, Formatter, Write};
use std::io;
use std::iter::repeat_n;

use crate::error::Result;
use crate::schema::expression::{Expression, KEYWORD_WIDTH};
use crate::view::object::{DocBody, ObjectDoc, Row, RowValue, Section};
use crate::view::{Color, NamedColor, Style, Styled, Table};
use serde::Serialize;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorMode {
    Enabled,
    Disabled,
}

#[derive(Clone, Copy)]
pub struct Ansi {
    mode: ColorMode,
}

const RESET: &str = "\x1b[0m";
const ROW_INDENT: &str = "  ";
const COLUMN_GAP: usize = 2;
const NESTED_INDENT: usize = 4;
const ENTRY_INDENT: usize = 2;

pub struct Tag {
    ansi: Ansi,
    style: Style,
    text: &'static str,
}

impl Display for Tag {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.ansi.fixed(self.style))?;
        f.write_str(self.text)?;
        f.write_str(self.ansi.reset())
    }
}

#[derive(Clone, Copy)]
pub enum Heading<'a> {
    Titled {
        object: &'a str,
        id: Option<&'a str>,
    },
    Untitled,
}

pub fn write_json_line(out: &mut dyn io::Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *out, value)?;
    out.write_all(b"\n")?;
    Ok(())
}

fn pad(out: &mut String, count: usize) {
    out.extend(repeat_n(' ', count));
}

impl Ansi {
    pub fn new(mode: ColorMode) -> Self {
        Ansi { mode }
    }

    const fn on(self) -> bool {
        matches!(self.mode, ColorMode::Enabled)
    }

    pub const fn reset(self) -> &'static str {
        if self.on() { RESET } else { "" }
    }
    pub const fn bold(self) -> &'static str {
        self.fixed(Style::Bold)
    }
    pub const fn dim(self) -> &'static str {
        self.fixed(Style::Dim)
    }
    pub const fn red(self) -> &'static str {
        self.fixed(Style::Failure)
    }
    pub const fn green(self) -> &'static str {
        self.fixed(Style::Success)
    }
    pub const fn heading(self) -> &'static str {
        self.fixed(Style::Heading)
    }
    pub const fn cyan(self) -> &'static str {
        self.fixed(Style::Accent)
    }

    pub const fn error_tag(self) -> Tag {
        Tag {
            ansi: self,
            style: Style::ErrorTag,
            text: "error:",
        }
    }

    pub const fn warning_tag(self) -> Tag {
        Tag {
            ansi: self,
            style: Style::WarningTag,
            text: "warning:",
        }
    }

    pub fn eprintln(self, styled: &Styled) {
        let mut line = String::new();
        self.paint(styled, &mut line);
        eprintln!("{line}");
    }

    pub fn paint(self, styled: &Styled, out: &mut String) {
        out.reserve(styled.text().len());
        if !self.on() {
            out.push_str(styled.text());
            return;
        }
        for (text, style) in styled.runs() {
            if style == Style::Plain {
                out.push_str(text);
            } else {
                Self::open(style, out);
                out.push_str(text);
                out.push_str(RESET);
            }
        }
    }

    pub fn paint_doc(self, doc: &ObjectDoc<'_>, heading: Heading<'_>, out: &mut String) {
        match heading {
            Heading::Titled { object, id } => {
                out.push_str(self.bold());
                out.push_str(doc.variant_label.unwrap_or(object));
                out.push_str(self.reset());
                match (doc.label, id) {
                    (Some(label), Some(id)) => {
                        out.push(' ');
                        out.push_str(self.heading());
                        out.push_str(label);
                        out.push_str(self.reset());
                        out.push(' ');
                        out.push_str(self.dim());
                        out.push_str("(id ");
                        out.push_str(id);
                        out.push(')');
                        out.push_str(self.reset());
                    }
                    (None, Some(id)) => {
                        out.push(' ');
                        out.push_str(self.heading());
                        out.push_str(id);
                        out.push_str(self.reset());
                    }
                    (_, None) => {}
                }
                out.push_str("\n\n");
            }
            Heading::Untitled => {
                if let Some(label) = doc.variant_label {
                    self.type_line(out, 0, label);
                    out.push('\n');
                }
            }
        }
        match &doc.body {
            DocBody::Raw(value) => {
                let _ = writeln!(out, "{value:#}");
            }
            DocBody::Sections(sections) => self.sections(out, sections, 0),
        }
    }

    pub fn table_header(self, out: &mut String, indent: usize, header: &[&str], widths: &[usize]) {
        pad(out, indent);
        out.push_str(self.bold());
        let mut pending = 0;
        for (label, width) in header.iter().zip(widths) {
            pad(out, pending);
            out.push_str(label);
            pending = width.saturating_sub(label.chars().count()) + COLUMN_GAP;
        }
        out.push_str(self.reset());
        out.push('\n');
    }

    pub fn table_row(self, out: &mut String, indent: usize, row: &[Styled], widths: &[usize]) {
        let mut pending = indent;
        for (cell, width) in row.iter().zip(widths) {
            if !cell.text().is_empty() {
                pad(out, pending);
                self.paint(cell, out);
                pending = 0;
            }
            pending += width.saturating_sub(cell.width()) + COLUMN_GAP;
        }
        out.push('\n');
    }

    fn nested_doc(self, out: &mut String, doc: &ObjectDoc<'_>, indent: usize) {
        if let Some(label) = doc.variant_label {
            self.type_line(out, indent, label);
        }
        self.nested_body(out, doc, indent);
    }

    fn nested_body(self, out: &mut String, doc: &ObjectDoc<'_>, indent: usize) {
        match &doc.body {
            DocBody::Raw(value) => {
                pad(out, indent);
                let _ = writeln!(out, "{value}");
            }
            DocBody::Sections(sections) => self.sections(out, sections, indent),
        }
    }

    fn type_line(self, out: &mut String, indent: usize, label: &str) {
        pad(out, indent);
        out.push_str("Type: ");
        out.push_str(label);
        out.push('\n');
    }

    fn sections(self, out: &mut String, sections: &[Section<'_>], indent: usize) {
        for (index, section) in sections.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            if let Some(title) = section.title {
                pad(out, indent);
                out.push_str(if indent == 0 {
                    self.heading()
                } else {
                    self.bold()
                });
                out.push_str(title);
                out.push_str(self.reset());
                out.push('\n');
            }
            let label_width = section
                .rows
                .iter()
                .map(|row| row.label.chars().count())
                .max()
                .unwrap_or(0);
            for row in &section.rows {
                self.row(out, row, label_width, indent);
            }
        }
    }

    fn row(self, out: &mut String, row: &Row<'_>, label_width: usize, indent: usize) {
        pad(out, indent);
        out.push_str(ROW_INDENT);
        out.push_str(row.label);
        out.push(':');
        let inner = indent + NESTED_INDENT;
        let align = label_width.saturating_sub(row.label.chars().count()) + 1;
        match &row.value {
            RowValue::Inline(value) => {
                pad(out, align);
                self.paint(value, out);
                out.push('\n');
            }
            RowValue::Nested(
                doc @ ObjectDoc {
                    variant_label: Some(label),
                    ..
                },
            ) => {
                pad(out, align);
                out.push_str(label);
                out.push('\n');
                self.nested_body(out, doc, inner);
            }
            RowValue::Block(text) => {
                out.push('\n');
                for line in text.lines() {
                    if !line.is_empty() {
                        pad(out, inner);
                        out.push_str(line);
                    }
                    out.push('\n');
                }
            }
            RowValue::Nested(doc) => {
                out.push('\n');
                self.nested_doc(out, doc, inner);
            }
            RowValue::Table(table) => {
                out.push('\n');
                self.doc_table(out, table, inner);
            }
            RowValue::Entries(entries) => {
                out.push('\n');
                for entry in entries {
                    pad(out, inner);
                    out.push_str(self.dim());
                    out.push_str(entry.key);
                    out.push(':');
                    out.push_str(self.reset());
                    out.push('\n');
                    self.nested_doc(out, &entry.doc, inner + ENTRY_INDENT);
                }
            }
            RowValue::Expression(expression) => {
                out.push('\n');
                self.expression(out, expression, inner);
            }
            RowValue::Empty => {
                pad(out, align);
                out.push_str(self.dim());
                out.push_str("<empty>");
                out.push_str(self.reset());
                out.push('\n');
            }
        }
    }

    fn expression(self, out: &mut String, expression: &Expression<&str>, indent: usize) {
        for (part, text) in expression.parts() {
            let keyword = part.keyword();
            let mut lines = text.split('\n');
            pad(out, indent);
            out.push_str(self.fixed(Style::Accent));
            out.push_str(keyword);
            out.push_str(self.reset());
            pad(out, KEYWORD_WIDTH.saturating_sub(keyword.len()));
            out.push_str(lines.next().unwrap_or_default());
            out.push('\n');
            for line in lines {
                pad(out, indent + KEYWORD_WIDTH);
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    fn doc_table(self, out: &mut String, table: &Table<'_>, indent: usize) {
        let widths = table.widths();
        self.table_header(out, indent, &table.header, &widths);
        for row in &table.rows {
            self.table_row(out, indent, row, &widths);
        }
    }

    const fn fixed(self, style: Style) -> &'static str {
        match Self::escape(style) {
            Some(code) if self.on() => code,
            _ => "",
        }
    }

    fn open(style: Style, out: &mut String) {
        match (style, Self::escape(style)) {
            (_, Some(code)) => out.push_str(code),
            (Style::Badge(Color::Rgb(r, g, b)), None) => {
                let _ = write!(out, "\x1b[38;2;{r};{g};{b}m");
            }
            (_, None) => {}
        }
    }

    const fn escape(style: Style) -> Option<&'static str> {
        Some(match style {
            Style::Plain | Style::Badge(Color::Rgb(..)) => return None,
            Style::Bold => "\x1b[1m",
            Style::ErrorTag => "\x1b[1;31m",
            Style::WarningTag => "\x1b[1;33m",
            Style::Heading => "\x1b[1;36m",
            Style::Dim => "\x1b[2m",
            Style::Failure | Style::Badge(Color::Named(NamedColor::Red)) => "\x1b[31m",
            Style::Success | Style::Badge(Color::Named(NamedColor::Green)) => "\x1b[32m",
            Style::Badge(Color::Named(NamedColor::Yellow)) => "\x1b[33m",
            Style::Badge(Color::Default | Color::Named(NamedColor::Blue)) => "\x1b[34m",
            Style::Accent | Style::Badge(Color::Named(NamedColor::Cyan)) => "\x1b[36m",
            Style::Badge(Color::Named(NamedColor::Black)) => "\x1b[30m",
            Style::Badge(Color::Named(NamedColor::Magenta)) => "\x1b[35m",
            Style::Badge(Color::Named(NamedColor::White)) => "\x1b[37m",
        })
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/cli/term/tests.rs"]
mod tests;
