/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::fmt::Write;

use unicode_width::UnicodeWidthStr;

use crate::schema::object::FieldSet;
use crate::schema::{Field, FieldType, ObjectDef, Schema};
use crate::view::styled::Styled;
use crate::view::value::ValueFormatter;
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug)]
pub struct Column<'a> {
    pub name: &'a str,
    pub label: &'a str,
    pub typ: Option<&'a FieldType>,
}

#[derive(Debug, Default)]
pub struct Table<'a> {
    pub header: Vec<&'a str>,
    pub rows: Vec<Vec<Styled>>,
}

impl<'a> Column<'a> {
    pub const ID: Column<'static> = Column {
        name: "id",
        label: "Id",
        typ: None,
    };

    pub fn list(def: ObjectDef<'a>, fields: FieldSet<'a>) -> Option<Vec<Column<'a>>> {
        let list = def.list()?;
        let mut columns = Vec::with_capacity(list.columns.len() + 1);
        columns.push(Column::ID);
        columns.extend(
            list.columns
                .iter()
                .filter(|col| col.name != Column::ID.name)
                .map(|col| Column {
                    name: &col.name,
                    label: &col.label,
                    typ: fields.get(&col.name).map(|f| &f.typ),
                }),
        );
        Some(columns)
    }

    pub fn named(
        schema: &'a Schema,
        def: ObjectDef<'a>,
        name: &'a str,
        field: Option<&'a Field>,
    ) -> Self {
        if name == Column::ID.name {
            return Column::ID;
        }
        Column {
            name,
            label: Self::label_for(schema, def, name).unwrap_or(name),
            typ: field.map(|f| &f.typ),
        }
    }

    fn label_for(schema: &'a Schema, def: ObjectDef<'a>, field: &str) -> Option<&'a str> {
        let object = def.name();
        let form_label = |key: &str| {
            schema
                .forms
                .get(key)?
                .sections
                .iter()
                .flat_map(|section| &section.fields)
                .find(|ff| ff.name == field && !ff.label.is_empty())
                .map(|ff| ff.label.as_str())
        };
        let list_label = |key: &str| {
            schema
                .lists
                .get(key)?
                .columns
                .iter()
                .find(|col| col.name == field)
                .map(|col| col.label.as_str())
        };
        let single = schema
            .schemas
            .get(object)
            .and_then(|shape| shape.single_name());
        let schema_names = single.into_iter().chain(
            def.variants()
                .iter()
                .filter_map(|variant| variant.schema_name.as_deref()),
        );
        form_label(object)
            .or_else(|| schema_names.into_iter().find_map(form_label))
            .or_else(|| schema.views_of(object).find_map(form_label))
            .or_else(|| list_label(object))
            .or_else(|| schema.views_of(object).find_map(list_label))
    }

    pub fn cells<'r>(
        columns: &'r [Column<'_>],
        row: &'r Map<String, Value>,
        values: &'r ValueFormatter<'_>,
    ) -> impl Iterator<Item = Styled> + 'r {
        columns.iter().map(move |col| {
            let mut cell = Styled::default();
            col.cell(&mut cell, row, values);
            cell
        })
    }

    pub fn cell(&self, out: &mut Styled, row: &Map<String, Value>, values: &ValueFormatter<'_>) {
        let Some(value) = row.get(self.name) else {
            return;
        };
        match (self.typ, value) {
            (Some(typ), _) => values.inline(out, value, typ),
            (None, Value::String(text)) => out.plain(text),
            (None, _) => {
                let _ = write!(out, "{value}");
            }
        }
    }
}

impl Table<'_> {
    pub fn widths(&self) -> Vec<usize> {
        let mut widths: Vec<usize> = self.header.iter().map(|label| label.width()).collect();
        for row in &self.rows {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.width());
            }
        }
        widths
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/view/table/tests.rs"]
mod tests;
