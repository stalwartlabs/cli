/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::cmp::Ordering;
use std::collections::HashSet;

use crate::schema::expression::Expression;
use crate::schema::{
    Field, FieldType, Fields, Form, MapValueType, ObjectDef, ObjectVariant, ScalarType, Schema,
};
use crate::view::labels::LabelCache;
use crate::view::styled::Styled;
use crate::view::table::{Column, Table};
use crate::view::value::ValueFormatter;
use serde_json::{Map, Value};

const OBJECT_LIST_KEY: &str = "#";
const MAP_KEY: &str = "Key";

#[derive(Debug)]
pub struct ObjectDoc<'a> {
    pub label: Option<&'a str>,
    pub variant_label: Option<&'a str>,
    pub body: DocBody<'a>,
}

#[derive(Debug)]
pub enum DocBody<'a> {
    Sections(Vec<Section<'a>>),
    Raw(&'a Value),
}

#[derive(Debug)]
pub struct Section<'a> {
    pub title: Option<&'a str>,
    pub rows: Vec<Row<'a>>,
}

#[derive(Debug)]
pub struct Row<'a> {
    pub label: &'a str,
    pub value: RowValue<'a>,
}

#[derive(Debug)]
pub enum RowValue<'a> {
    Inline(Styled),
    Block(&'a str),
    Nested(ObjectDoc<'a>),
    Table(Table<'a>),
    Entries(Vec<Entry<'a>>),
    Expression(Expression<&'a str>),
    Empty,
}

#[derive(Debug)]
pub struct Entry<'a> {
    pub key: &'a str,
    pub doc: ObjectDoc<'a>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Depth {
    Top,
    Nested,
}

struct Shape<'a> {
    fields: Option<&'a Fields>,
    form: Option<&'a Form>,
    variant_label: Option<&'a str>,
}

struct Builder<'a, 'l> {
    schema: &'a Schema,
    values: ValueFormatter<'l>,
}

impl<'a> ObjectDoc<'a> {
    pub fn build(
        schema: &'a Schema,
        labels: &LabelCache,
        object: ObjectDef<'a>,
        value: &'a Value,
    ) -> Self {
        let mut doc = Builder {
            schema,
            values: ValueFormatter::new(schema, labels),
        }
        .doc(object.name(), value, Depth::Top);
        doc.label = object
            .label_property()
            .and_then(|property| value.get(property)?.as_str())
            .filter(|label| !label.is_empty());
        doc
    }
}

impl<'a> Builder<'a, '_> {
    fn doc(&self, object_name: &'a str, value: &'a Value, depth: Depth) -> ObjectDoc<'a> {
        let Some(obj) = value.as_object() else {
            return ObjectDoc {
                label: None,
                variant_label: None,
                body: DocBody::Raw(value),
            };
        };
        let shape = self.shape(object_name, obj);
        let body = match shape.fields {
            Some(fields) => DocBody::Sections(self.sections(object_name, fields, shape.form, obj)),
            None if depth == Depth::Nested && shape.variant_label.is_some() => {
                DocBody::Sections(Vec::new())
            }
            None => DocBody::Raw(value),
        };
        ObjectDoc {
            label: None,
            variant_label: shape.variant_label,
            body,
        }
    }

    fn shape(&self, object_name: &str, obj: &Map<String, Value>) -> Shape<'a> {
        let Some(shape) = self.schema.schemas.get(object_name) else {
            return Shape {
                fields: None,
                form: None,
                variant_label: None,
            };
        };
        if let Some(schema_name) = shape.single_name() {
            return Shape {
                fields: self.schema.fields.get(schema_name),
                form: self.form(object_name, schema_name),
                variant_label: None,
            };
        }
        let variant = shape.variant_of(obj);
        let schema_name = variant.and_then(|v| v.schema_name.as_deref());
        Shape {
            fields: schema_name.and_then(|name| self.schema.fields.get(name)),
            form: self.variant_form(object_name, variant, schema_name),
            variant_label: variant.map(|v| v.label.as_str()),
        }
    }

    fn form(&self, object_name: &str, schema_name: &str) -> Option<&'a Form> {
        let forms = &self.schema.forms;
        forms
            .get(object_name)
            .or_else(|| forms.get(schema_name))
            .or_else(|| {
                self.schema
                    .views_of(object_name)
                    .find_map(|view| forms.get(view))
            })
    }

    fn variant_form(
        &self,
        object_name: &str,
        variant: Option<&ObjectVariant>,
        schema_name: Option<&str>,
    ) -> Option<&'a Form> {
        let forms = &self.schema.forms;
        schema_name
            .and_then(|name| forms.get(name))
            .or_else(|| {
                let variant = variant?;
                let mut key = String::with_capacity(object_name.len() + 1 + variant.name.len());
                key.push_str(object_name);
                key.push('/');
                key.push_str(&variant.name);
                forms.get(&key)
            })
            .or_else(|| forms.get(object_name))
    }

    fn sections(
        &self,
        object_name: &'a str,
        fields: &'a Fields,
        form: Option<&'a Form>,
        obj: &'a Map<String, Value>,
    ) -> Vec<Section<'a>> {
        let mut forms = form
            .into_iter()
            .chain(
                self.schema
                    .views_of(object_name)
                    .filter_map(|view| self.schema.forms.get(view)),
            )
            .peekable();
        if forms.peek().is_none() {
            let rows = self.rows(fields.properties.iter(), obj, |_| true);
            return Self::section(None, rows).into_iter().collect();
        }

        let mut seen: HashSet<&str> = HashSet::new();
        let mut sections = Vec::new();
        for section in forms.flat_map(|form| &form.sections) {
            let rows = section
                .fields
                .iter()
                .filter(|ff| ff.name != "@type")
                .filter_map(|ff| {
                    let field = fields.properties.get(&ff.name)?;
                    let value = obj.get(&ff.name)?;
                    seen.insert(ff.name.as_str())
                        .then(|| self.row(ff.label_or_name(), field, value))
                })
                .collect();
            sections.extend(Self::section(section.title.as_deref(), rows));
        }
        let orphans = self.rows(fields.properties.iter(), obj, |name| !seen.contains(name));
        sections.extend(Self::section(None, orphans));
        sections
    }

    fn rows(
        &self,
        properties: impl Iterator<Item = (&'a String, &'a Field)>,
        obj: &'a Map<String, Value>,
        include: impl Fn(&str) -> bool,
    ) -> Vec<Row<'a>> {
        properties
            .filter(|(name, _)| include(name))
            .filter_map(|(name, field)| Some(self.row(name, field, obj.get(name)?)))
            .collect()
    }

    fn section(title: Option<&'a str>, rows: Vec<Row<'a>>) -> Option<Section<'a>> {
        (!rows.is_empty()).then(|| Section {
            title: title.filter(|t| !t.is_empty()),
            rows,
        })
    }

    fn row(&self, label: &'a str, field: &'a Field, value: &'a Value) -> Row<'a> {
        let block = match &field.typ {
            FieldType::String { format, .. } if format.is_multiline() => {
                value.as_str().filter(|text| text.contains('\n'))
            }
            _ => None,
        };
        if let Some(text) = block {
            return Row {
                label,
                value: RowValue::Block(text),
            };
        }
        let value = match &field.typ {
            FieldType::Object { object_name, .. } if self.schema.is_expression(object_name) => {
                match Expression::parse(value).filter(|expression| !expression.rules.is_empty()) {
                    Some(expression) => RowValue::Expression(expression),
                    None => self.inline(value, &field.typ),
                }
            }
            FieldType::Object { object_name, .. } if !value.is_null() => {
                RowValue::Nested(self.doc(object_name, value, Depth::Nested))
            }
            FieldType::ObjectList { object_name, .. } if !value.is_null() => {
                self.object_list(object_name, value)
            }
            FieldType::Map {
                key_class,
                value_class: MapValueType::Object { object_name },
                ..
            } if !value.is_null() => self.object_map(key_class, object_name, value),
            typ => self.inline(value, typ),
        };
        Row { label, value }
    }

    fn inline(&self, value: &Value, typ: &FieldType) -> RowValue<'a> {
        let mut inline = Styled::default();
        self.values.inline(&mut inline, value, typ);
        RowValue::Inline(inline)
    }

    fn object_list(&self, object_name: &'a str, value: &'a Value) -> RowValue<'a> {
        let map = match value.as_object() {
            Some(map) if map.is_empty() => return RowValue::Empty,
            Some(map) => map,
            None => return RowValue::Nested(self.doc(object_name, value, Depth::Nested)),
        };
        let Some(columns) = self.columns(object_name) else {
            let mut items: Vec<(&str, &Value)> = map.iter().map(|(k, v)| (k.as_str(), v)).collect();
            items.sort_by(|a, b| natural_cmp(a.0, b.0));
            return self.entries(object_name, items);
        };
        let mut rows: Vec<(&str, &Map<String, Value>)> = map
            .iter()
            .filter_map(|(k, v)| Some((k.as_str(), v.as_object()?)))
            .collect();
        rows.sort_by(|a, b| natural_cmp(a.0, b.0));
        RowValue::Table(self.table(
            OBJECT_LIST_KEY,
            &columns,
            rows.into_iter().map(|(key, row)| {
                let mut cell = Styled::default();
                cell.plain(key);
                (cell, row)
            }),
        ))
    }

    fn object_map(
        &self,
        key_class: &ScalarType,
        object_name: &'a str,
        value: &'a Value,
    ) -> RowValue<'a> {
        let map = match value.as_object() {
            Some(map) if map.is_empty() => return RowValue::Empty,
            Some(map) => map,
            None => return RowValue::Nested(self.doc(object_name, value, Depth::Nested)),
        };
        let Some(columns) = self.columns(object_name) else {
            let mut items: Vec<(&str, &Value)> = map.iter().map(|(k, v)| (k.as_str(), v)).collect();
            items.sort_by(|a, b| a.0.cmp(b.0));
            return self.entries(object_name, items);
        };
        let mut rows: Vec<(Styled, &Map<String, Value>)> = map
            .iter()
            .filter_map(|(k, v)| {
                let row = v.as_object()?;
                let mut key = Styled::default();
                self.values.scalar_key(&mut key, key_class, k);
                Some((key, row))
            })
            .collect();
        rows.sort_by(|a, b| a.0.text().cmp(b.0.text()));
        RowValue::Table(self.table(MAP_KEY, &columns, rows.into_iter()))
    }

    fn entries(&self, object_name: &'a str, items: Vec<(&'a str, &'a Value)>) -> RowValue<'a> {
        RowValue::Entries(
            items
                .into_iter()
                .map(|(key, value)| Entry {
                    key,
                    doc: self.doc(object_name, value, Depth::Nested),
                })
                .collect(),
        )
    }

    fn columns(&self, object_name: &str) -> Option<Vec<Column<'a>>> {
        let schema_name = self.schema.schemas.get(object_name)?.single_name()?;
        let fields = self.schema.fields.get(schema_name)?;
        let columns: Vec<Column<'a>> = match self.form(object_name, schema_name) {
            Some(form) => form
                .sections
                .iter()
                .flat_map(|section| &section.fields)
                .filter(|ff| ff.name != "@type")
                .filter_map(|ff| {
                    let field = fields.properties.get(&ff.name)?;
                    (!field.typ.is_nested_object()).then(|| Column {
                        name: &ff.name,
                        label: ff.label_or_name(),
                        typ: Some(&field.typ),
                    })
                })
                .collect(),
            None => fields
                .properties
                .iter()
                .filter(|(_, field)| !field.typ.is_nested_object())
                .map(|(name, field)| Column {
                    name,
                    label: name,
                    typ: Some(&field.typ),
                })
                .collect(),
        };
        (!columns.is_empty()).then_some(columns)
    }

    fn table(
        &self,
        key_header: &'a str,
        columns: &[Column<'a>],
        rows: impl ExactSizeIterator<Item = (Styled, &'a Map<String, Value>)>,
    ) -> Table<'a> {
        let mut table = Table {
            header: Vec::with_capacity(columns.len() + 1),
            rows: Vec::with_capacity(rows.len()),
        };
        table.header.push(key_header);
        table.header.extend(columns.iter().map(|col| col.label));
        for (key, row) in rows {
            let mut cells = Vec::with_capacity(columns.len() + 1);
            cells.push(key);
            cells.extend(Column::cells(columns, row, &self.values));
            table.rows.push(cells);
        }
        table
    }
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        _ => a.cmp(b),
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/view/object/tests.rs"]
mod tests;
