/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::Schema;
use crate::tui::form::expression::ExpressionView;
use crate::tui::form::map::{MapShape, MapView};
use crate::tui::form::model::{FormModel, Rules, Shape, item_summary};
use crate::tui::form::view::{FormAction, FormView};
use serde_json::{Map, Value};
use std::fmt::Write;

pub const AT_MOST: &str = "This list is full";
pub const AT_LEAST: &str = "This list cannot have fewer items";
pub const READ_ONLY: &str = "This list is read-only";

#[derive(Debug)]
pub enum Frame {
    Form {
        slot: Slot,
        label: Box<str>,
        view: Box<FormView>,
    },
    List {
        name: Box<str>,
        label: Box<str>,
        list: ListView,
    },
    Map {
        name: Box<str>,
        label: Box<str>,
        map: Box<MapView>,
    },
    Expression {
        name: Box<str>,
        label: Box<str>,
        expression: Box<ExpressionView>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    Field(Box<str>),
    Item { index: usize, fresh: bool },
    Entry(usize),
}

#[derive(Debug)]
pub struct ListView {
    pub object: Box<str>,
    pub items: Vec<Map<String, Value>>,
    pub cursor: usize,
    pub status: Option<&'static str>,
    pub dirty: bool,
    min: usize,
    max: Option<usize>,
    rules: Rules,
}

pub enum ListStep {
    None,
    Open(Frame),
    Back,
    Save,
}

impl Frame {
    pub fn object(
        schema: &Schema,
        can_get: &dyn Fn(&str) -> bool,
        object: &str,
        name: &str,
        label: &str,
        value: Option<&Value>,
        rules: Rules,
    ) -> Self {
        let values = match value {
            Some(Value::Object(map)) => map.clone(),
            _ => schema.new_object_value(object, None),
        };
        Frame::Form {
            slot: Slot::Field(name.into()),
            label: label.into(),
            view: Box::new(embedded(schema, can_get, object, values, rules)),
        }
    }

    pub fn list(
        object: &str,
        name: &str,
        label: &str,
        value: Option<&Value>,
        (min, max): (Option<usize>, Option<usize>),
        rules: Rules,
    ) -> Self {
        Frame::List {
            name: name.into(),
            label: label.into(),
            list: ListView {
                object: object.into(),
                items: list_items(value),
                cursor: 0,
                status: None,
                dirty: false,
                min: min.unwrap_or(0),
                max,
                rules,
            },
        }
    }

    pub fn map(
        name: &str,
        label: &str,
        shape: &MapShape<'_>,
        value: Option<&Value>,
        rules: Rules,
    ) -> Self {
        Frame::Map {
            name: name.into(),
            label: label.into(),
            map: Box::new(MapView::new(shape, value, rules)),
        }
    }

    pub fn expression(
        name: &str,
        label: &str,
        value: Option<&Value>,
        error: Option<&str>,
        rules: Rules,
    ) -> Self {
        Frame::Expression {
            name: name.into(),
            label: label.into(),
            expression: Box::new(ExpressionView::new(value, error, rules)),
        }
    }

    pub fn changed(&self) -> bool {
        match self {
            Frame::Form { view, .. } => view.has_changes(),
            Frame::List { list, .. } => list.dirty,
            Frame::Map { map, .. } => map.dirty,
            Frame::Expression { expression, .. } => expression.changed(),
        }
    }

    pub fn crumb(&self, out: &mut String) {
        match self {
            Frame::Form {
                slot: Slot::Item { index, .. },
                ..
            } => {
                let _ = write!(out, " › #{}", index + 1);
            }
            Frame::Form { label, .. }
            | Frame::List { label, .. }
            | Frame::Map { label, .. }
            | Frame::Expression { label, .. } => {
                out.push_str(" › ");
                out.push_str(label);
            }
        }
    }
}

impl ListView {
    pub fn act(
        &mut self,
        schema: &Schema,
        can_get: &dyn Fn(&str) -> bool,
        action: FormAction,
    ) -> ListStep {
        self.status = None;
        let len = self.items.len();
        if self.rules.read_only
            && matches!(
                action,
                FormAction::Add
                    | FormAction::Delete
                    | FormAction::Clear
                    | FormAction::MoveUp
                    | FormAction::MoveDown
            )
        {
            self.status = Some(READ_ONLY);
            return ListStep::None;
        }
        match action {
            FormAction::Down
            | FormAction::Up
            | FormAction::First
            | FormAction::Last
            | FormAction::PageUp
            | FormAction::PageDown => {
                if let Some(cursor) = action.moved(self.cursor, len) {
                    self.cursor = cursor;
                }
            }
            FormAction::Edit => return self.open(schema, can_get, self.cursor, false),
            FormAction::Add => {
                if self.max.is_some_and(|max| len >= max) {
                    self.status = Some(AT_MOST);
                } else {
                    self.items.push(schema.new_object_value(&self.object, None));
                    self.cursor = len;
                    return self.open(schema, can_get, len, true);
                }
            }
            FormAction::Delete | FormAction::Clear => {
                if len <= self.min {
                    self.status = Some(AT_LEAST);
                } else if self.cursor < len {
                    self.items.remove(self.cursor);
                    self.cursor = self.cursor.min(len.saturating_sub(2));
                    self.dirty = true;
                }
            }
            FormAction::MoveUp if self.cursor > 0 && self.cursor < len => {
                self.items.swap(self.cursor, self.cursor - 1);
                self.cursor -= 1;
                self.dirty = true;
            }
            FormAction::MoveDown if self.cursor + 1 < len => {
                self.items.swap(self.cursor, self.cursor + 1);
                self.cursor += 1;
                self.dirty = true;
            }
            FormAction::Back => return ListStep::Back,
            FormAction::Save => return ListStep::Save,
            _ => {}
        }
        ListStep::None
    }

    fn open(
        &self,
        schema: &Schema,
        can_get: &dyn Fn(&str) -> bool,
        index: usize,
        fresh: bool,
    ) -> ListStep {
        let Some(item) = self.items.get(index) else {
            return ListStep::None;
        };
        ListStep::Open(Frame::Form {
            slot: Slot::Item { index, fresh },
            label: Box::default(),
            view: Box::new(embedded(
                schema,
                can_get,
                &self.object,
                item.clone(),
                self.rules,
            )),
        })
    }

    pub fn close_item(&mut self, index: usize, fresh: bool, view: FormView) {
        if view.has_changes() {
            if let Some(item) = self.items.get_mut(index) {
                *item = view.into_values();
                self.dirty = true;
            }
        } else if fresh && index < self.items.len() {
            self.items.remove(index);
            self.cursor = self.cursor.min(self.items.len().saturating_sub(1));
        }
    }

    pub fn into_value(self) -> Value {
        Value::Object(
            self.items
                .into_iter()
                .enumerate()
                .map(|(index, item)| (index.to_string(), Value::Object(item)))
                .collect(),
        )
    }

    pub fn item_title(&self, schema: &Schema, item: &Map<String, Value>, out: &mut String) {
        let variant = schema
            .schemas
            .get(&*self.object)
            .and_then(|shape| shape.variant_of(item));
        if let Some(variant) = variant {
            out.push_str(if variant.label.is_empty() {
                &variant.name
            } else {
                &variant.label
            });
        }
        if let Some(summary) = item_summary(schema, &self.object, item) {
            if !out.is_empty() {
                out.push_str(" · ");
            }
            out.push_str(summary);
        }
        if out.is_empty() {
            out.push_str("(empty)");
        }
    }
}

pub fn embedded(
    schema: &Schema,
    can_get: &dyn Fn(&str) -> bool,
    object: &str,
    values: Map<String, Value>,
    rules: Rules,
) -> FormView {
    FormView::new(FormModel::new(
        schema,
        Shape::Embedded(object.into()),
        rules,
        values.clone(),
        values,
        can_get,
    ))
}

fn list_items(value: Option<&Value>) -> Vec<Map<String, Value>> {
    let object = |value: &Value| match value {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    match value {
        Some(Value::Object(map)) => {
            let mut items: Vec<(usize, &Value)> = map
                .iter()
                .map(|(key, value)| (key.parse().unwrap_or(usize::MAX), value))
                .collect();
            items.sort_by_key(|(index, _)| *index);
            items.into_iter().map(|(_, value)| object(value)).collect()
        }
        Some(Value::Array(values)) => values.iter().map(object).collect(),
        _ => Vec::new(),
    }
}
