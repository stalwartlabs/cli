/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::AT_TYPE;
use crate::schema::object::FieldSet;
use crate::tui::job::objects::Upload;
use crate::view::styled::Styled;
use crate::view::value::ValueFormatter;
use ratatui::text::Line;
use serde_json::{Map, Value};
use std::fmt::Write;
use std::mem;

const CELL_WIDTH: usize = 28;
const BLOB_PREFIX: &str = "blob";
const PROPERTY_WIDTH: usize = 34;
const DIFF_CELL_WIDTH: usize = 19;
const DIFF_PROPERTY_WIDTH: usize = 20;
const DIFF_LINE_WIDTH: usize = 64;
const PROPERTY_HEADER: &str = "PROPERTY";

pub fn blob_uploads(map: &mut Map<String, Value>, blobs: &[Box<str>]) -> Vec<Upload> {
    let mut uploads = Vec::new();
    for name in blobs {
        let Some(value) = map.get_mut(&**name) else {
            continue;
        };
        if !value.is_string() {
            continue;
        }
        let creation_id = format!("{BLOB_PREFIX}{}", uploads.len());
        let reference = Value::String(format!("#{creation_id}"));
        if let Value::String(text) = mem::replace(value, reference) {
            uploads.push(Upload {
                creation_id: creation_id.into(),
                text,
            });
        }
    }
    uploads
}

pub fn variant_changed(original: &Map<String, Value>, values: &Map<String, Value>) -> bool {
    original
        .get(AT_TYPE)
        .is_some_and(|before| values.get(AT_TYPE).is_some_and(|after| after != before))
}

#[derive(Clone, Copy)]
pub struct Cells<'a> {
    pub formatter: ValueFormatter<'a>,
    pub fields: FieldSet<'a>,
}

impl Cells<'_> {
    fn write(&self, name: &str, value: &Value, width: usize, out: &mut String) {
        let Some(field) = self.fields.get(name) else {
            cell(value, width, out);
            return;
        };
        let start = out.len();
        let mut styled = Styled::default();
        self.formatter.inline(&mut styled, value, &field.typ);
        out.push_str(&styled.into_text());
        cut(out, start, width);
    }
}

fn properties(count: usize) -> &'static str {
    if count == 1 { "property" } else { "properties" }
}

pub fn update_lines(
    cells: Cells<'_>,
    name: &str,
    original: &Map<String, Value>,
    patch: &Map<String, Value>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(patch.len() + 4);
    lines.push(Line::raw(format!(
        "Update {name}  ({} {})",
        patch.len(),
        properties(patch.len())
    )));
    lines.push(Line::raw(""));
    let width = patch
        .keys()
        .map(|path| path.chars().count())
        .max()
        .unwrap_or_default()
        .clamp(PROPERTY_HEADER.len(), DIFF_PROPERTY_WIDTH + 1);
    let widths = Widths {
        property: width,
        cell: DIFF_CELL_WIDTH + 1,
    };
    lines.push(Line::raw(widths.row(
        "  ",
        PROPERTY_HEADER,
        "BEFORE",
        "AFTER",
    )));
    let mut property = String::new();
    let mut before = String::new();
    let mut after = String::new();
    for (path, value) in patch {
        property.clear();
        before.clear();
        after.clear();
        property.push_str(path);
        if path.chars().count() > DIFF_PROPERTY_WIDTH {
            cut(&mut property, 0, DIFF_LINE_WIDTH);
            lines.push(Line::raw(format!("~ {property}")));
            property.clear();
        }
        match pointer(original, path) {
            Some(old) => cells.write(path, old, DIFF_CELL_WIDTH, &mut before),
            None => before.push('-'),
        }
        cells.write(path, value, DIFF_CELL_WIDTH, &mut after);
        let marker = if property.is_empty() { "  " } else { "~ " };
        lines.push(Line::raw(widths.row(marker, &property, &before, &after)));
    }
    lines.push(Line::raw(""));
    lines.push(Line::raw(
        "Unchanged secrets and server-set fields are not part of the patch.",
    ));
    lines
}

pub fn create_lines(
    cells: Cells<'_>,
    name: &str,
    payload: &Map<String, Value>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(payload.len() + 3);
    lines.push(Line::raw(format!(
        "Create {name}  ({} {})",
        payload.len(),
        properties(payload.len())
    )));
    lines.push(Line::raw(""));
    let mut text = String::new();
    for (name, value) in payload {
        text.clear();
        cells.write(name, value, CELL_WIDTH, &mut text);
        lines.push(Line::raw(CREATE_WIDTHS.row("+ ", name, &text, "")));
    }
    lines
}

pub fn json_lines(object: &str, args: &Value) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw(format!("{object}/set")), Line::raw("")];
    let text = serde_json::to_string_pretty(args).unwrap_or_default();
    lines.extend(text.lines().map(|line| Line::raw(line.to_string())));
    lines
}

pub fn created_doc(created: Option<&Value>, at_type: Option<&str>) -> Option<Value> {
    let Some(Value::Object(created)) = created else {
        return None;
    };
    let mut doc: Map<String, Value> = created
        .iter()
        .filter(|(key, _)| !hashify::set!(key.as_bytes(), "id", "blobId"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if doc.is_empty() {
        return None;
    }
    if let Some(at_type) = at_type {
        doc.entry(AT_TYPE)
            .or_insert_with(|| Value::String(at_type.into()));
    }
    Some(Value::Object(doc))
}

pub fn action_result(
    input: &Map<String, Value>,
    created: Option<&Value>,
    at_type: Option<&str>,
) -> Option<Value> {
    let extras = match created {
        Some(Value::Object(created)) => Some(created),
        _ => None,
    };
    let mut doc: Map<String, Value> = input
        .iter()
        .filter(|(key, _)| key.as_str() != AT_TYPE)
        .chain(
            extras
                .into_iter()
                .flatten()
                .filter(|(key, _)| !hashify::set!(key.as_bytes(), "id", "blobId")),
        )
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if doc.is_empty() {
        return None;
    }
    if let Some(at_type) = at_type {
        doc.insert(AT_TYPE.into(), Value::String(at_type.into()));
    }
    Some(Value::Object(doc))
}

pub fn created_text(created: Option<&Value>) -> Option<String> {
    let Some(Value::Object(created)) = created else {
        return None;
    };
    let mut text = String::new();
    for (key, value) in created {
        if hashify::set!(key.as_bytes(), "id", "blobId") {
            continue;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        match value {
            Value::String(value) => {
                let _ = write!(text, "{key}: {value}");
            }
            value => {
                let _ = write!(text, "{key}: {value}");
            }
        }
    }
    (!text.is_empty()).then_some(text)
}

fn pointer<'a>(original: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    let mut segments = path.split('/');
    let mut current = original.get(&unescape(segments.next()?))?;
    for segment in segments {
        current = match current {
            Value::Object(map) => map.get(&unescape(segment))?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn unescape(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

fn cell(value: &Value, width: usize, out: &mut String) {
    let start = out.len();
    let _ = write!(out, "{value}");
    cut(out, start, width);
}

fn cut(out: &mut String, start: usize, width: usize) {
    if let Some((at, _)) = out
        .get(start..)
        .and_then(|text| text.char_indices().nth(width))
    {
        out.truncate(start + at);
        out.push('…');
    }
}

struct Widths {
    property: usize,
    cell: usize,
}

const CREATE_WIDTHS: Widths = Widths {
    property: PROPERTY_WIDTH,
    cell: CELL_WIDTH,
};

impl Widths {
    fn row(&self, marker: &str, property: &str, before: &str, after: &str) -> String {
        let Widths { property: pw, cell } = *self;
        format!("{marker}{property:<pw$} {before:<cell$} {after}")
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/form/save/tests.rs"]
mod tests;
