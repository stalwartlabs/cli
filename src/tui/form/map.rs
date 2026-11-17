/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::{FieldType, MapValueType, ScalarType, Schema};
use crate::tui::form::editor::{Inline, Kind};
use crate::tui::form::model::Rules;
use crate::tui::form::nested::{AT_LEAST, AT_MOST, Frame, READ_ONLY, Slot, embedded};
use crate::tui::form::view::{EditAction, FormAction, FormView, Pick, PickAt, PickKind};
use ratatui::crossterm::event::KeyEvent;
use serde_json::{Map, Value};
use std::borrow::Cow;

const DUPLICATE: &str = "This key is already in the map";
const EMPTY_KEY: &str = "Enter a key";
const KEY: &str = "Key";
const VALUE: &str = "Value";

#[derive(Debug)]
pub struct MapView {
    pub entries: Vec<(String, Value)>,
    pub cursor: usize,
    pub status: Option<Box<str>>,
    pub dirty: bool,
    pub key_type: FieldType,
    pub value_type: FieldType,
    pub key_label: Box<str>,
    pub value_label: Box<str>,
    editing: Option<MapEditing>,
    min: usize,
    max: Option<usize>,
    rules: Rules,
}

#[derive(Debug)]
pub enum MapEditing {
    Key(Inline),
    Value(Inline),
}

pub struct MapShape<'a> {
    pub key_class: &'a ScalarType,
    pub value_class: &'a MapValueType,
    pub items: (Option<usize>, Option<usize>),
    pub key_label: Option<&'a str>,
    pub value_label: Option<&'a str>,
}

pub enum MapStep {
    None,
    Open(Frame),
    Pick(Pick),
    Back,
    Save,
}

impl MapView {
    pub fn new(shape: &MapShape<'_>, value: Option<&Value>, rules: Rules) -> Self {
        let entries = match value {
            Some(Value::Object(map)) => map
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            _ => Vec::new(),
        };
        let (min, max) = shape.items;
        Self {
            entries,
            cursor: 0,
            status: None,
            dirty: false,
            key_type: shape.key_class.field_type(),
            value_type: shape.value_class.field_type(),
            key_label: shape.key_label.unwrap_or(KEY).into(),
            value_label: shape.value_label.unwrap_or(VALUE).into(),
            editing: None,
            min: min.unwrap_or(0),
            max,
            rules,
        }
    }

    pub fn entry_type(&self, schema: &Schema, key: &str) -> Cow<'_, FieldType> {
        if let FieldType::Enum { enum_name, .. } = &self.key_type
            && let FieldType::Number {
                min, max, nullable, ..
            } = &self.value_type
            && let Some(format) = schema
                .enum_variant(enum_name, key)
                .and_then(|variant| variant.value_format)
        {
            return Cow::Owned(FieldType::Number {
                format,
                min: min.clone(),
                max: max.clone(),
                nullable: *nullable,
            });
        }
        Cow::Borrowed(&self.value_type)
    }

    pub fn editing(&self) -> Option<&MapEditing> {
        self.editing.as_ref()
    }

    pub fn editing_mut(&mut self) -> Option<&mut MapEditing> {
        self.editing.as_mut()
    }

    pub fn act(
        &mut self,
        schema: &Schema,
        can_get: &dyn Fn(&str) -> bool,
        action: FormAction,
    ) -> MapStep {
        self.status = None;
        let len = self.entries.len();
        let mutating = matches!(
            action,
            FormAction::Add | FormAction::Delete | FormAction::Clear
        );
        if self.rules.read_only && mutating {
            self.status = Some(READ_ONLY.into());
            return MapStep::None;
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
            FormAction::Edit => return self.open_value(schema, can_get),
            FormAction::Add => {
                if self.max.is_some_and(|max| len >= max) {
                    self.status = Some(AT_MOST.into());
                    return MapStep::None;
                }
                return match &self.key_type {
                    FieldType::Enum { enum_name, .. } => MapStep::Pick(Pick {
                        at: PickAt::MapKey,
                        kind: PickKind::Enum {
                            name: enum_name.as_str().into(),
                            nullable: false,
                            multi: false,
                        },
                    }),
                    FieldType::ObjectId { object_name, .. } => MapStep::Pick(Pick {
                        at: PickAt::MapKey,
                        kind: PickKind::Reference {
                            object: object_name.as_str().into(),
                            nullable: false,
                            multi: false,
                        },
                    }),
                    typ => {
                        if let Some(kind) = Kind::of_field(typ) {
                            self.editing = Some(MapEditing::Key(Inline::open(kind, None)));
                        }
                        MapStep::None
                    }
                };
            }
            FormAction::Delete | FormAction::Clear => {
                if len <= self.min {
                    self.status = Some(AT_LEAST.into());
                } else if self.cursor < len {
                    self.entries.remove(self.cursor);
                    self.cursor = self.cursor.min(len.saturating_sub(2));
                    self.dirty = true;
                }
            }
            FormAction::Back => return MapStep::Back,
            FormAction::Save => return MapStep::Save,
            _ => {}
        }
        MapStep::None
    }

    fn open_value(&mut self, schema: &Schema, can_get: &dyn Fn(&str) -> bool) -> MapStep {
        let index = self.cursor;
        let Some((key, value)) = self.entries.get(index) else {
            return MapStep::None;
        };
        match &self.value_type {
            FieldType::Object { object_name, .. } => {
                let values = match value {
                    Value::Object(map) => map.clone(),
                    _ => schema.new_object_value(object_name, None),
                };
                MapStep::Open(Frame::Form {
                    slot: Slot::Entry(index),
                    label: key.as_str().into(),
                    view: Box::new(embedded(schema, can_get, object_name, values, self.rules)),
                })
            }
            _ if self.rules.read_only => MapStep::None,
            FieldType::Enum { enum_name, .. } => MapStep::Pick(Pick {
                at: PickAt::MapValue(index),
                kind: PickKind::Enum {
                    name: enum_name.as_str().into(),
                    nullable: false,
                    multi: false,
                },
            }),
            _ => {
                if let Some(kind) = Kind::of_field(&self.entry_type(schema, key)) {
                    self.editing = Some(MapEditing::Value(Inline::open(kind, Some(value))));
                }
                MapStep::None
            }
        }
    }

    fn add_key(&mut self, schema: &Schema, can_get: &dyn Fn(&str) -> bool, key: &str) -> MapStep {
        if key.is_empty() {
            self.status = Some(EMPTY_KEY.into());
            return MapStep::None;
        }
        if let Some(index) = self.entries.iter().position(|(known, _)| known == key) {
            self.cursor = index;
            self.status = Some(DUPLICATE.into());
            return MapStep::None;
        }
        let value = match &self.value_type {
            FieldType::String { .. } => Value::String(String::new()),
            FieldType::Number { min, .. } => min.clone().map_or(Value::from(0), Value::Number),
            FieldType::Enum { enum_name, .. } => schema
                .enums
                .get(enum_name)
                .and_then(|variants| variants.first())
                .map_or(Value::Null, |variant| Value::String(variant.name.clone())),
            FieldType::Object { object_name, .. } => {
                Value::Object(schema.new_object_value(object_name, None))
            }
            _ => Value::Null,
        };
        self.entries.push((key.to_owned(), value));
        self.cursor = self.entries.len() - 1;
        self.dirty = true;
        self.open_value(schema, can_get)
    }

    pub fn edit_key(
        &mut self,
        schema: &Schema,
        can_get: &dyn Fn(&str) -> bool,
        event: KeyEvent,
        action: Option<EditAction>,
    ) -> MapStep {
        let Some(editing) = &mut self.editing else {
            return MapStep::None;
        };
        let inline = match editing {
            MapEditing::Key(inline) | MapEditing::Value(inline) => inline,
        };
        match action {
            Some(EditAction::Cancel) => self.editing = None,
            Some(EditAction::Copy) => {}
            Some(EditAction::Commit | EditAction::Done) => return self.commit(schema, can_get),
            None => {
                inline.handle(event);
            }
        }
        MapStep::None
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(MapEditing::Key(inline) | MapEditing::Value(inline)) = &mut self.editing {
            inline.paste(text);
        }
    }

    fn commit(&mut self, schema: &Schema, can_get: &dyn Fn(&str) -> bool) -> MapStep {
        let Some(editing) = self.editing.take() else {
            return MapStep::None;
        };
        match editing {
            MapEditing::Key(inline) => match inline.commit() {
                Ok(Value::String(key)) => self.add_key(schema, can_get, &key),
                Ok(_) => MapStep::None,
                Err(message) => {
                    self.status = Some(message.into());
                    self.editing = Some(MapEditing::Key(inline));
                    MapStep::None
                }
            },
            MapEditing::Value(inline) => {
                match inline.commit() {
                    Ok(value) => {
                        if let Some((_, slot)) = self.entries.get_mut(self.cursor) {
                            *slot = value;
                            self.dirty = true;
                        }
                    }
                    Err(message) => {
                        self.status = Some(message.into());
                        self.editing = Some(MapEditing::Value(inline));
                    }
                }
                MapStep::None
            }
        }
    }

    pub fn apply_pick(
        &mut self,
        schema: &Schema,
        can_get: &dyn Fn(&str) -> bool,
        at: PickAt,
        key: &str,
    ) -> MapStep {
        match at {
            PickAt::MapKey => self.add_key(schema, can_get, key),
            PickAt::MapValue(index) => {
                if let Some((_, slot)) = self.entries.get_mut(index) {
                    *slot = Value::String(key.to_owned());
                    self.dirty = true;
                }
                MapStep::None
            }
            PickAt::Row(_) => MapStep::None,
        }
    }

    pub fn pick_target(&self, at: PickAt) -> (&str, Option<&Value>) {
        match at {
            PickAt::MapValue(index) => (
                &self.value_label,
                self.entries.get(index).map(|(_, value)| value),
            ),
            PickAt::MapKey | PickAt::Row(_) => (&self.key_label, None),
        }
    }

    pub fn close_entry(&mut self, index: usize, view: FormView) {
        if view.has_changes()
            && let Some((_, slot)) = self.entries.get_mut(index)
        {
            *slot = Value::Object(view.into_values());
            self.dirty = true;
        }
    }

    pub fn into_value(self) -> Value {
        Value::Object(self.entries.into_iter().collect::<Map<_, _>>())
    }
}
