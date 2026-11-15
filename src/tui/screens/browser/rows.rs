/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::query::PageResult;
use crate::schema::{AT_TYPE, Column, Schema};
use crate::tui::widgets::table::{MAX_CELL_WIDTH, RowCells};
use crate::view::{Color, LabelCache, Style, Styled, ValueFormatter};
use serde_json::{Map, Value};
use std::collections::HashMap;

const ID: &str = "id";

#[derive(Debug)]
pub struct RowBuilder {
    names: Box<[Box<str>]>,
    labels: Box<[Box<str>]>,
    label_property: Option<Box<str>>,
    properties: Box<[Box<str>]>,
}

impl RowBuilder {
    pub fn new(
        columns: &[Column],
        label_property: Option<&str>,
        discriminator: Option<&str>,
    ) -> Self {
        let mut properties: Vec<Box<str>> = Vec::with_capacity(columns.len() + 3);
        properties.push(ID.into());
        let wanted = columns
            .iter()
            .map(|c| c.name.as_str())
            .chain(label_property)
            .chain(discriminator);
        for name in wanted {
            if !properties.iter().any(|p| &**p == name) {
                properties.push(name.into());
            }
        }
        Self {
            names: columns.iter().map(|c| c.name.as_str().into()).collect(),
            labels: columns.iter().map(|c| c.label.as_str().into()).collect(),
            label_property: label_property.map(Into::into),
            properties: properties.into(),
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(AsRef::as_ref)
    }

    pub fn labels(&self) -> impl Iterator<Item = &str> {
        self.labels.iter().map(AsRef::as_ref)
    }

    pub fn properties(&self) -> &[Box<str>] {
        &self.properties
    }

    pub fn build(
        &self,
        schema: &Schema,
        labels: &LabelCache,
        object: &str,
        result: &PageResult,
    ) -> Vec<RowCells> {
        let def = schema.get_object(object);
        let formatter = ValueFormatter::new(schema, labels);
        let by_id: HashMap<&str, &Map<String, Value>> = result
            .objects
            .iter()
            .filter_map(|obj| Some((obj.get(ID)?.as_str()?, obj)))
            .collect();
        let empty = Map::new();
        result
            .ids
            .iter()
            .map(|id| {
                let obj = by_id.get(id.as_str()).copied().unwrap_or(&empty);
                let fields = def.and_then(|def| def.fields_of(obj));
                let cells = self
                    .names
                    .iter()
                    .map(|name| {
                        let mut cell = Styled::default();
                        let value = if &**name == ID {
                            Some(Value::String(id.clone()))
                        } else {
                            None
                        };
                        let value = value.as_ref().or_else(|| obj.get(&**name));
                        match (value, fields.and_then(|f| f.properties.get(&**name))) {
                            (None | Some(Value::Null), _) => {}
                            (Some(Value::String(at_type)), None) if &**name == AT_TYPE => {
                                let label = def
                                    .and_then(|def| def.variant(at_type))
                                    .map(|variant| variant.label.as_str())
                                    .filter(|label| !label.is_empty())
                                    .unwrap_or(at_type);
                                cell.push(label, Style::Badge(Color::Default));
                            }
                            (Some(value), Some(field)) => {
                                formatter.inline(&mut cell, value, &field.typ)
                            }
                            (Some(Value::String(text)), None) => cell.plain(text),
                            (Some(value), None) => cell.plain(&value.to_string()),
                        }
                        cell.tidy(usize::from(MAX_CELL_WIDTH));
                        cell
                    })
                    .collect();
                let label = self
                    .label_property
                    .as_deref()
                    .and_then(|p| obj.get(p))
                    .and_then(Value::as_str)
                    .filter(|label| !label.is_empty())
                    .unwrap_or(id);
                RowCells {
                    id: id.as_str().into(),
                    label: label.into(),
                    cells,
                }
            })
            .collect()
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/browser/rows/tests.rs"]
mod tests;
