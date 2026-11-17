/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::fmt::Write;

use crate::schema::expression::{Expression, Part};
use crate::schema::{FieldType, MapValueType, NumberFormat, ScalarType, Schema};
use crate::view::labels::LabelCache;
use crate::view::styled::{Color, Style, Styled};
use serde_json::Value;

const SIZE_UNITS: [(&str, f64); 4] = [
    ("TB", (1u64 << 40) as f64),
    ("GB", (1u64 << 30) as f64),
    ("MB", (1u64 << 20) as f64),
    ("KB", 1024.0),
];

const SUMMARY_ITEMS: usize = 3;

const DURATION_UNITS: [(u64, &str); 5] = [
    (86_400_000, "d"),
    (3_600_000, "h"),
    (60_000, "m"),
    (1_000, "s"),
    (1, "ms"),
];

#[derive(Clone, Copy)]
pub struct ValueFormatter<'a> {
    schema: &'a Schema,
    labels: &'a LabelCache,
}

impl<'a> ValueFormatter<'a> {
    pub fn new(schema: &'a Schema, labels: &'a LabelCache) -> Self {
        ValueFormatter { schema, labels }
    }

    pub fn inline(&self, out: &mut Styled, value: &Value, field_type: &FieldType) {
        if value.is_null() {
            Self::none(out);
            return;
        }
        match field_type {
            FieldType::String { .. } | FieldType::UtcDateTime { .. } => Self::string(out, value),
            FieldType::Number { format, .. } => Self::number(out, value, format),
            FieldType::Boolean => match value.as_bool() {
                Some(true) => out.push("Yes", Style::Success),
                Some(false) => out.push("No", Style::Dim),
                None => Self::raw(out, value),
            },
            FieldType::Enum { enum_name, .. } => self.enum_value(out, enum_name, value),
            FieldType::BlobId => {
                out.plain("<blob: ");
                if let Some(s) = value.as_str() {
                    out.plain(s);
                }
                out.plain(">");
            }
            FieldType::ObjectId { object_name, .. } => match value.as_str() {
                Some(id) => self.object_id(out, object_name, id),
                None => Self::raw(out, value),
            },
            FieldType::Set { class, .. } => self.set(out, value, class),
            FieldType::Map {
                key_class,
                value_class,
                ..
            } => self.map(out, value, key_class, value_class),
            FieldType::Object { object_name, .. } if self.schema.is_expression(object_name) => {
                match Expression::parse(value) {
                    Some(expression) => Self::expression(out, &expression),
                    None => Self::raw(out, value),
                }
            }
            FieldType::Object { object_name, .. } => {
                out.plain(self.variant_label(object_name, value).unwrap_or("<object>"))
            }
            FieldType::ObjectList { object_name, .. } => match value {
                Value::Object(map) => self.items(out, object_name, map.values(), map.len()),
                Value::Array(items) => self.items(out, object_name, items.iter(), items.len()),
                _ => Self::raw(out, value),
            },
        }
    }

    pub fn scalar_key(&self, out: &mut Styled, class: &ScalarType, key: &str) {
        match class {
            ScalarType::String { .. } => out.plain(key),
            ScalarType::ObjectId { object_name } => self.object_id(out, object_name, key),
            ScalarType::Enum { enum_name } => self.enum_name(out, enum_name, key),
        }
    }

    fn variant_label(&self, object_name: &str, value: &Value) -> Option<&'a str> {
        let obj = value.as_object()?;
        let variant = self.schema.schemas.get(object_name)?.variant_of(obj)?;
        Some(&variant.label)
    }

    fn set(&self, out: &mut Styled, value: &Value, class: &ScalarType) {
        let Some(map) = value.as_object() else {
            Self::raw(out, value);
            return;
        };
        let mut keys = map.keys();
        let Some(first) = keys.next() else {
            Self::none(out);
            return;
        };
        self.scalar_key(out, class, first);
        for key in keys {
            out.plain(", ");
            self.scalar_key(out, class, key);
        }
    }

    fn map(&self, out: &mut Styled, value: &Value, key_class: &ScalarType, class: &MapValueType) {
        let Some(map) = value.as_object() else {
            Self::raw(out, value);
            return;
        };
        if map.is_empty() {
            Self::none(out);
            return;
        }
        if matches!(class, MapValueType::Object { .. }) {
            for (index, key) in map.keys().take(SUMMARY_ITEMS).enumerate() {
                if index > 0 {
                    out.plain(", ");
                }
                self.scalar_key(out, key_class, key);
            }
            Self::more(out, map.len());
            return;
        }
        for (index, (key, item)) in map.iter().enumerate() {
            if index > 0 {
                out.plain(", ");
            }
            self.scalar_key(out, key_class, key);
            out.plain(" → ");
            self.map_value(out, key_class, key, class, item);
        }
    }

    fn map_value(
        &self,
        out: &mut Styled,
        key_class: &ScalarType,
        key: &str,
        class: &MapValueType,
        value: &Value,
    ) {
        if value.is_null() {
            Self::none(out);
            return;
        }
        match class {
            MapValueType::String { .. } => Self::string(out, value),
            MapValueType::Number { format, .. } => {
                let variant_format = match key_class {
                    ScalarType::Enum { enum_name } => self
                        .schema
                        .enum_variant(enum_name, key)
                        .and_then(|variant| variant.value_format),
                    _ => None,
                };
                Self::number(out, value, &variant_format.unwrap_or(*format))
            }
            MapValueType::Enum { enum_name } => self.enum_value(out, enum_name, value),
            MapValueType::Object { .. } => out.plain("<object>"),
        }
    }

    fn enum_value(&self, out: &mut Styled, enum_name: &str, value: &Value) {
        match value.as_str() {
            Some(name) => self.enum_name(out, enum_name, name),
            None => Self::raw(out, value),
        }
    }

    fn enum_name(&self, out: &mut Styled, enum_name: &str, name: &str) {
        match self.schema.enum_variant(enum_name, name) {
            Some(v) => {
                let color = v.color.as_deref().map_or(Color::Default, Color::parse);
                out.push(&v.label, Style::Badge(color));
            }
            None => out.plain(name),
        }
    }

    fn object_id(&self, out: &mut Styled, object_name: &str, id: &str) {
        match self.labels.get(object_name, id) {
            Some(label) => {
                out.plain(label);
                out.push(" (", Style::Dim);
                out.push(id, Style::Dim);
                out.push(")", Style::Dim);
            }
            None => out.plain(id),
        }
    }

    fn string(out: &mut Styled, value: &Value) {
        match value.as_str() {
            Some(s) => Self::text(out, s),
            None => Self::raw(out, value),
        }
    }

    fn items<'v>(
        &self,
        out: &mut Styled,
        object_name: &str,
        items: impl Iterator<Item = &'v Value> + Clone,
        len: usize,
    ) {
        if len == 0 {
            Self::none(out);
            return;
        }
        let shown = items.take(SUMMARY_ITEMS);
        if !shown
            .clone()
            .all(|item| self.item_label(object_name, item).is_some())
        {
            let _ = write!(out, "{len} item{}", if len == 1 { "" } else { "s" });
            return;
        }
        for (index, label) in shown
            .filter_map(|item| self.item_label(object_name, item))
            .enumerate()
        {
            if index > 0 {
                out.plain(", ");
            }
            out.plain(label);
        }
        Self::more(out, len);
    }

    fn item_label<'s>(&'s self, object_name: &str, item: &'s Value) -> Option<&'s str> {
        let obj = item.as_object()?;
        let shape = self.schema.schemas.get(object_name)?;
        if let Some(variant) = shape.variant_of(obj) {
            return Some(&variant.label);
        }
        self.schema
            .fields
            .get(shape.single_name()?)?
            .properties
            .iter()
            .filter(|(_, field)| {
                !field.is_server_set()
                    && !field.typ.is_secret_scalar()
                    && matches!(field.typ, FieldType::String { .. })
            })
            .find_map(|(name, _)| obj.get(name)?.as_str().filter(|text| !text.is_empty()))
    }

    fn more(out: &mut Styled, len: usize) {
        if let Some(rest) = len.checked_sub(SUMMARY_ITEMS).filter(|rest| *rest > 0) {
            let _ = write!(out, " +{rest} more");
        }
    }

    fn expression(out: &mut Styled, expression: &Expression<&str>) {
        if expression.rules.is_empty() {
            Self::text(out, expression.default);
            return;
        }
        for (part, text) in expression.parts() {
            match part {
                Part::Condition(0) => {}
                Part::Condition(_) | Part::Default => out.push(" · ", Style::Dim),
                Part::Result(_) => out.plain(" "),
            }
            out.push(part.keyword(), Style::Accent);
            out.plain(" ");
            Self::text(out, text);
        }
    }

    fn text(out: &mut Styled, text: &str) {
        if text.is_empty() {
            out.push("<empty>", Style::Dim);
        } else {
            out.plain(text);
        }
    }

    fn none(out: &mut Styled) {
        out.push("<none>", Style::Dim);
    }

    fn raw(out: &mut Styled, value: &Value) {
        let _ = write!(out, "{value}");
    }

    fn number(out: &mut Styled, value: &Value, format: &NumberFormat) {
        let Some(n) = value.as_f64() else {
            Self::raw(out, value);
            return;
        };
        match format {
            NumberFormat::Size => Self::size(out, n),
            NumberFormat::Duration => Self::duration(out, n),
            _ => {
                let _ = match value.as_i64() {
                    Some(i) => write!(out, "{i}"),
                    None => write!(out, "{n}"),
                };
            }
        }
    }

    fn size(out: &mut Styled, bytes: f64) {
        if bytes <= 0.0 {
            out.plain("0 B");
            return;
        }
        let _ = match SIZE_UNITS.iter().find(|(_, unit)| bytes >= *unit) {
            Some((label, unit)) if bytes / unit < 100.0 => {
                write!(out, "{:.1} {label}", bytes / unit)
            }
            Some((label, unit)) => write!(out, "{:.0} {label}", bytes / unit),
            None => write!(out, "{bytes:.0} B"),
        };
    }

    fn duration(out: &mut Styled, ms: f64) {
        let ms_abs = ms.abs() as u64;
        let mut units = DURATION_UNITS.iter().skip_while(|(unit, _)| ms_abs < *unit);
        let Some((unit, label)) = units.next() else {
            out.plain("0 ms");
            return;
        };
        let sign = if ms.is_sign_negative() { "-" } else { "" };
        let _ = write!(out, "{sign}{} {label}", ms_abs / unit);
        let rem = ms_abs % unit;
        if rem > 0
            && let Some((next_unit, next_label)) = units.next()
            && rem / next_unit > 0
        {
            let _ = write!(out, " {} {next_label}", rem / next_unit);
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/view/value/tests.rs"]
mod tests;
