/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::{AT_TYPE, Field, FieldType, ScalarType, Schema, StringFormat};
use crate::tui::form::editor::sieve::sievepad_link;
use crate::tui::form::editor::textarea::TextArea;
use crate::tui::form::editor::{Inline, Kind};
use crate::tui::form::expression::ExpressionStep;
use crate::tui::form::map::{MapShape, MapStep};
use crate::tui::form::model::{
    Access, FormModel, MAX_NESTING, OTP_URL, PASSKEY_REGISTRATION, Row, RowKind,
};
use crate::tui::form::nested::{Frame, ListStep, Slot};
use crate::tui::form::otp::{OTP_AUTH, OtpEditor, TotpSetup};
use crate::tui::keymap::{Context, Hint, code, ctrl, key, keys, shifted, upper};
use crate::view::time::UtcTimestamp;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde_json::{Map, Value};
use std::io;
use std::time::SystemTime;

const SCRIPT_NAME: &str = "name";
const ISO_LEN: usize = 24;
const PAGE_ROWS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormAction {
    Down,
    Up,
    First,
    Last,
    PageUp,
    PageDown,
    NextSection,
    PrevSection,
    Edit,
    Toggle,
    Revert,
    Clear,
    Save,
    External,
    Delete,
    Add,
    MoveUp,
    MoveDown,
    Now,
    Open,
    Copy,
    Back,
}

impl FormAction {
    pub fn moved(self, cursor: usize, len: usize) -> Option<usize> {
        let last = len.saturating_sub(1);
        let target = match self {
            FormAction::Down => cursor + 1,
            FormAction::Up => cursor.saturating_sub(1),
            FormAction::First => 0,
            FormAction::Last => last,
            FormAction::PageDown => cursor + PAGE_ROWS,
            FormAction::PageUp => cursor.saturating_sub(PAGE_ROWS),
            _ => return None,
        };
        Some(target.min(last))
    }
}

keys! { pub KEYS, pub HINTS: FormAction;
    "j/k" Down "field" true [key('j'), code(KeyCode::Down)],
    "k" Up "previous field" false [key('k'), code(KeyCode::Up)],
    "g/G" First "first / last field" false [key('g'), code(KeyCode::Home)],
    "G" Last "last field" false [upper('G'), code(KeyCode::End)],
    "pgdn" PageDown "page down" false [code(KeyCode::PageDown), ctrl('f')],
    "pgup" PageUp "page up" false [code(KeyCode::PageUp), ctrl('b')],
    "tab" NextSection "section" true [code(KeyCode::Tab)],
    "⇧tab" PrevSection "previous section" false [shifted(KeyCode::BackTab), code(KeyCode::BackTab)],
    "⏎" Edit "edit / choose" true [code(KeyCode::Enter)],
    "space" Toggle "toggle" true [code(KeyCode::Char(' '))],
    "u" Revert "revert field" true [key('u')],
    "x" Clear "clear field" false [key('x')],
    "^s" Save "review & save" true [ctrl('s'), upper('S')],
    "E" External "edit as JSON in $EDITOR" true [upper('E')],
    "d" Delete "delete" false [key('d')],
    "a" Add "add list item" false [key('a')],
    "K" MoveUp "move item up" false [upper('K')],
    "J" MoveDown "move item down" false [upper('J')],
    "t" Now "set date to now" false [key('t')],
    "o" Open "open in Sievepad / web interface" false [key('o')],
    "y" Copy "copy value or Sievepad link" false [key('y')],
    "esc" Back "back" true [code(KeyCode::Esc), code(KeyCode::Backspace)],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditAction {
    Commit,
    Done,
    Copy,
    Cancel,
}

keys! { pub EDIT_KEYS, pub EDIT_HINTS: EditAction;
    "⏎" Commit "done" true [code(KeyCode::Enter)],
    "^s" Done "done" false [ctrl('s')],
    "^y" Copy "copy (otpauth URL)" false [ctrl('y')],
    "esc" Cancel "cancel" true [code(KeyCode::Esc)],
}

const fn hint(label: &'static str, desc: &'static str, footer: bool) -> Hint {
    Hint {
        label,
        desc,
        footer,
    }
}

pub const TEXT_HINTS: &[Hint] = &[
    hint("^s", "done", true),
    hint("⏎", "new line", true),
    hint("esc", "cancel", true),
];

pub const LIST_HINTS: &[Hint] = &[
    hint("⏎", "next item (one per line)", true),
    hint("^s", "done", true),
    hint("esc", "cancel", true),
];

pub const VIEW_HINTS: &[Hint] = &[hint("↑↓", "scroll", true), hint("esc", "close", true)];

pub const LIST_FRAME_HINTS: &[Hint] = &[
    hint("j/k", "item", true),
    hint("⏎", "edit", true),
    hint("a", "add", true),
    hint("d", "remove", true),
    hint("J/K", "move", true),
    hint("^s", "review & save", true),
    hint("esc", "done", true),
];

pub const EXPRESSION_FRAME_HINTS: &[Hint] = &[
    hint("j/k", "part", true),
    hint("tab", "rule", true),
    hint("g/G", "first / default value", false),
    hint("⏎", "edit", true),
    hint("a", "add rule", true),
    hint("d", "remove rule", true),
    hint("J/K", "move rule", true),
    hint("x", "clear part", false),
    hint("u", "revert expression", false),
    hint("y", "copy part", false),
    hint("^s", "review & save", true),
    hint("esc", "done", true),
];

pub const MAP_FRAME_HINTS: &[Hint] = &[
    hint("j/k", "entry", true),
    hint("⏎", "edit value", true),
    hint("a", "add", true),
    hint("d", "remove", true),
    hint("^s", "review & save", true),
    hint("esc", "done", true),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickKind {
    Enum {
        name: Box<str>,
        nullable: bool,
        multi: bool,
    },
    Reference {
        object: Box<str>,
        nullable: bool,
        multi: bool,
    },
    Variant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickAt {
    Row(usize),
    MapKey,
    MapValue(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub at: PickAt,
    pub kind: PickKind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FormEvent {
    None,
    Pick(Pick),
    Save,
    External,
    Delete,
    Passkey,
    Back,
}

#[derive(Debug)]
enum Editing {
    Inline(Inline),
    Lines(TextArea),
    Json(TextArea),
    View(TextArea),
    Otp(Box<OtpEditor>),
}

#[derive(Debug)]
pub struct FormView {
    pub model: FormModel,
    pub row: usize,
    pub scroll: usize,
    editing: Option<Editing>,
    stack: Vec<Frame>,
    totp: Option<Box<TotpSetup>>,
    rejected: bool,
}

enum Step {
    Event(FormEvent),
    Open(Frame),
}

enum Opener {
    Inline(Kind),
    Toggle,
    Pick(PickKind),
    Lines,
    Json,
    Otp,
    Passkey,
    Object(Box<str>),
    Expression,
    List {
        object: Box<str>,
        items: (Option<usize>, Option<usize>),
    },
    Map,
    Unsupported,
}

impl FormView {
    pub fn new(model: FormModel) -> Self {
        let row = model.first_error_row().unwrap_or(0);
        Self {
            model,
            row,
            scroll: 0,
            editing: None,
            stack: Vec::new(),
            totp: None,
            rejected: false,
        }
    }

    pub fn at_input(model: FormModel) -> Self {
        let mut view = Self::new(model);
        if view.model.first_error_row().is_none()
            && let Some(row) = view.model.first_input_row()
        {
            view.row = row;
        }
        view
    }

    pub fn active(&self) -> &FormView {
        match self.stack.last() {
            Some(Frame::Form { view, .. }) => view,
            Some(Frame::List { .. } | Frame::Map { .. } | Frame::Expression { .. }) | None => self,
        }
    }

    fn active_mut(&mut self) -> Option<&mut FormView> {
        if self.stack.is_empty() {
            return Some(self);
        }
        match self.stack.last_mut() {
            Some(Frame::Form { view, .. }) => Some(view),
            Some(Frame::List { .. } | Frame::Map { .. } | Frame::Expression { .. }) | None => None,
        }
    }

    pub fn pick_target(&self, pick: &Pick) -> Option<(&str, Option<&Value>)> {
        match (pick.at, self.stack.last()) {
            (PickAt::Row(row), _) => {
                let view = self.active();
                let row = view.model.layout.rows.get(row)?;
                Some((&row.label, view.model.value(&row.name)))
            }
            (at, Some(Frame::Map { map, .. })) => Some(map.pick_target(at)),
            (PickAt::MapKey | PickAt::MapValue(_), _) => None,
        }
    }

    pub fn frames(&self) -> &[Frame] {
        &self.stack
    }

    pub fn top_frame_mut(&mut self) -> Option<&mut Frame> {
        self.stack.last_mut()
    }

    pub fn has_changes(&self) -> bool {
        self.model.has_changes() || self.stack.iter().any(Frame::changed)
    }

    pub fn into_values(mut self) -> Map<String, Value> {
        self.collapse();
        self.model.values
    }

    pub fn collapse(&mut self) {
        while !self.stack.is_empty() {
            self.pop();
        }
    }

    fn pop(&mut self) {
        let Some(frame) = self.stack.pop() else {
            return;
        };
        match frame {
            Frame::Form {
                slot: Slot::Field(name),
                view,
                ..
            } => {
                if view.has_changes() {
                    self.set_top(&name, Value::Object(view.into_values()));
                }
            }
            Frame::Form {
                slot: Slot::Item { index, fresh },
                view,
                ..
            } => {
                if let Some(Frame::List { list, .. }) = self.stack.last_mut() {
                    list.close_item(index, fresh, *view);
                }
            }
            Frame::Form {
                slot: Slot::Entry(index),
                view,
                ..
            } => {
                if let Some(Frame::Map { map, .. }) = self.stack.last_mut() {
                    map.close_entry(index, *view);
                }
            }
            Frame::List { name, list, .. } => {
                if list.dirty {
                    self.set_top(&name, list.into_value());
                }
            }
            Frame::Map { name, map, .. } => {
                if map.dirty {
                    self.set_top(&name, map.into_value());
                }
            }
            Frame::Expression {
                name, expression, ..
            } => {
                if expression.changed() {
                    self.set_top(&name, expression.into_value());
                }
            }
        }
    }

    fn set_top(&mut self, name: &str, value: Value) {
        match self.stack.last_mut() {
            Some(Frame::Form { view, .. }) => view.model.set(name, value),
            Some(Frame::List { .. } | Frame::Map { .. } | Frame::Expression { .. }) => {}
            None => self.model.set(name, value),
        }
    }

    pub fn edit_context(&self) -> Option<Context> {
        match self.stack.last() {
            Some(Frame::Map { map, .. }) => {
                return Some(
                    map.editing()
                        .map_or(Context::FormMap, |_| Context::FormEdit),
                );
            }
            Some(Frame::List { .. }) => return Some(Context::FormList),
            Some(Frame::Expression { expression, .. }) => {
                return Some(if expression.is_editing() {
                    Context::FormEditText
                } else {
                    Context::FormExpression
                });
            }
            _ => {}
        }
        Some(match self.active().editing.as_ref()? {
            Editing::Inline(inline) if inline.is_multiline() => Context::FormEditText,
            Editing::Inline(_) | Editing::Otp(_) => Context::FormEdit,
            Editing::Json(_) => Context::FormEditText,
            Editing::Lines(_) => Context::FormEditLines,
            Editing::View(_) => Context::FormViewing,
        })
    }

    pub fn is_editing(&self) -> bool {
        match self.stack.last() {
            Some(Frame::Map { map, .. }) => map.editing().is_some(),
            Some(Frame::Expression { expression, .. }) => expression.is_editing(),
            _ => self.active().editing.is_some(),
        }
    }

    pub fn current_name(&self) -> Option<&str> {
        let view = self.active();
        view.model.layout.rows.get(view.row).map(|r| &*r.name)
    }

    pub fn sievepad(&self, schema: &Schema) -> Option<io::Result<String>> {
        let view = self.active();
        let row = view.model.layout.rows.get(view.row)?;
        let Some(Field {
            typ:
                FieldType::String {
                    format: StringFormat::Sieve,
                    ..
                },
            ..
        }) = view.model.field(schema, &row.name)
        else {
            return None;
        };
        let source = view.model.value(&row.name).and_then(Value::as_str)?;
        let name = view
            .model
            .value(SCRIPT_NAME)
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(&row.label);
        Some(sievepad_link(name, source))
    }

    pub fn current_field<'s>(&self, schema: &'s Schema) -> Option<&'s Field> {
        let view = self.active();
        view.model.field(schema, view.current_name()?)
    }

    pub fn act(
        &mut self,
        schema: &Schema,
        can_get: &dyn Fn(&str) -> bool,
        action: FormAction,
    ) -> FormEvent {
        let step = match self.stack.last_mut() {
            None => self.act_here(schema, can_get, action),
            Some(Frame::Form { view, .. }) => match view.act_here(schema, can_get, action) {
                Step::Event(FormEvent::Back) => {
                    self.pop();
                    return FormEvent::None;
                }
                Step::Event(FormEvent::External | FormEvent::Delete) => {
                    return FormEvent::None;
                }
                step => step,
            },
            Some(Frame::List { list, .. }) => match list.act(schema, can_get, action) {
                ListStep::None => return FormEvent::None,
                ListStep::Open(frame) => Step::Open(frame),
                ListStep::Back => {
                    self.pop();
                    return FormEvent::None;
                }
                ListStep::Save => Step::Event(FormEvent::Save),
            },
            Some(Frame::Map { map, .. }) => match map.act(schema, can_get, action) {
                MapStep::None => return FormEvent::None,
                MapStep::Open(frame) => Step::Open(frame),
                MapStep::Pick(pick) => Step::Event(FormEvent::Pick(pick)),
                MapStep::Back => {
                    self.pop();
                    return FormEvent::None;
                }
                MapStep::Save => Step::Event(FormEvent::Save),
            },
            Some(Frame::Expression { expression, .. }) => match expression.act(action) {
                ExpressionStep::None => return FormEvent::None,
                ExpressionStep::Back => {
                    self.pop();
                    return FormEvent::None;
                }
                ExpressionStep::Save => Step::Event(FormEvent::Save),
            },
        };
        match step {
            Step::Open(frame) => {
                self.push(frame);
                FormEvent::None
            }
            Step::Event(FormEvent::Save) => {
                self.collapse();
                FormEvent::Save
            }
            Step::Event(event) => event,
        }
    }

    fn push(&mut self, frame: Frame) {
        if self.stack.len() < MAX_NESTING {
            self.stack.push(frame);
        }
    }

    fn map_step(&mut self, step: MapStep) {
        if let MapStep::Open(frame) = step {
            self.push(frame);
        }
    }

    fn act_here(
        &mut self,
        schema: &Schema,
        can_get: &dyn Fn(&str) -> bool,
        action: FormAction,
    ) -> Step {
        if let Some(row) = action.moved(self.row, self.model.layout.rows.len()) {
            self.row = row;
            return Step::Event(FormEvent::None);
        }
        match action {
            FormAction::Down
            | FormAction::Up
            | FormAction::First
            | FormAction::Last
            | FormAction::PageUp
            | FormAction::PageDown => {}
            FormAction::NextSection | FormAction::PrevSection => {
                let sections = &self.model.layout.sections;
                let current = self.model.layout.section_of(self.row).unwrap_or(0);
                let target = if action == FormAction::NextSection {
                    (current + 1) % sections.len().max(1)
                } else {
                    (current + sections.len().max(1) - 1) % sections.len().max(1)
                };
                if let Some(section) = sections.get(target) {
                    self.row = section.rows.start;
                }
            }
            FormAction::Edit => return self.open(schema, can_get),
            FormAction::Toggle => self.toggle(schema),
            FormAction::Revert => {
                if let Some(name) = self.editable_name() {
                    self.model.revert(&name);
                    if &*name == AT_TYPE {
                        self.model.relayout(schema, can_get);
                    }
                }
            }
            FormAction::Clear => {
                if let Some(name) = self.editable_name()
                    && self
                        .model
                        .field(schema, &name)
                        .is_some_and(|f| f.typ.is_nullable())
                {
                    self.model.set(&name, Value::Null);
                }
            }
            FormAction::Save => return Step::Event(FormEvent::Save),
            FormAction::External => return Step::Event(FormEvent::External),
            FormAction::Delete => return Step::Event(FormEvent::Delete),
            FormAction::Back => return Step::Event(FormEvent::Back),
            FormAction::Now => self.set_now(schema),
            FormAction::Add
            | FormAction::MoveUp
            | FormAction::MoveDown
            | FormAction::Open
            | FormAction::Copy => {}
        }
        Step::Event(FormEvent::None)
    }

    fn editable_name(&self) -> Option<Box<str>> {
        let row = self.model.layout.rows.get(self.row)?;
        (row.access == Access::Edit).then(|| row.name.clone())
    }

    fn set_now(&mut self, schema: &Schema) {
        let Some(name) = self.editable_name() else {
            return;
        };
        if let Some(Field {
            typ: FieldType::UtcDateTime { .. },
            ..
        }) = self.model.field(schema, &name)
        {
            let mut now = String::with_capacity(ISO_LEN);
            if UtcTimestamp::from_system(SystemTime::now())
                .format_iso(&mut now)
                .is_ok()
            {
                self.model.set(&name, Value::String(now));
            }
        }
    }

    pub fn copy_text(&self) -> Option<String> {
        if let Some(Frame::Expression { expression, .. }) = self.stack.last() {
            return expression.copy_text();
        }
        let view = self.active();
        if let Some(Editing::Otp(editor)) = &view.editing {
            return editor.url().map(str::to_owned);
        }
        let row = view.model.layout.rows.get(view.row)?;
        match view.model.value(&row.name)? {
            Value::Null => None,
            Value::String(text) => Some(text.clone()),
            Value::Object(map) if map.contains_key(OTP_URL) => {
                map.get(OTP_URL)?.as_str().map(str::to_owned)
            }
            value => serde_json::to_string_pretty(value).ok(),
        }
    }

    pub fn on_passkey(&self, schema: &Schema) -> bool {
        matches!(
            self.current_field(schema).map(|field| &field.typ),
            Some(FieldType::Object { object_name, .. }) if object_name == PASSKEY_REGISTRATION
        )
    }

    fn toggle(&mut self, schema: &Schema) {
        let Some(name) = self.editable_name() else {
            return;
        };
        if let Some(Field {
            typ: FieldType::Boolean,
            ..
        }) = self.model.field(schema, &name)
        {
            let on = self.model.value(&name).and_then(Value::as_bool) == Some(true);
            self.model.set(&name, Value::Bool(!on));
        }
    }

    fn open(&mut self, schema: &Schema, can_get: &dyn Fn(&str) -> bool) -> Step {
        let Some(row) = self.model.layout.rows.get(self.row) else {
            return Step::Event(FormEvent::None);
        };
        if row.kind == RowKind::Variant && row.access == Access::Edit {
            return Step::Event(FormEvent::Pick(Pick {
                at: PickAt::Row(self.row),
                kind: PickKind::Variant,
            }));
        }
        let Some(field) = self.model.field(schema, &row.name) else {
            return Step::Event(FormEvent::None);
        };
        let value = self.model.value(&row.name);
        if row.access != Access::Edit || row.kind == RowKind::Variant {
            return self.view_row(schema, can_get, &field.typ);
        }
        match opener(schema, &field.typ) {
            Opener::Inline(kind) => self.editing = Some(Editing::Inline(Inline::open(kind, value))),
            Opener::Toggle => self.toggle(schema),
            Opener::Pick(kind) => {
                return Step::Event(FormEvent::Pick(Pick {
                    at: PickAt::Row(self.row),
                    kind,
                }));
            }
            Opener::Lines => {
                let mut text = String::new();
                if let Some(Value::Object(set)) = value {
                    for (i, key) in set.keys().enumerate() {
                        if i > 0 {
                            text.push('\n');
                        }
                        text.push_str(key);
                    }
                }
                self.editing = Some(Editing::Lines(TextArea::new(&text)));
            }
            Opener::Otp => match OtpEditor::open(value, &mut self.totp) {
                Ok(editor) => self.editing = Some(Editing::Otp(Box::new(editor))),
                Err(err) => {
                    let name = row.name.clone();
                    self.model.set_error(&name, err.to_string());
                }
            },
            Opener::Passkey => return Step::Event(FormEvent::Passkey),
            Opener::Object(object) => {
                let (name, label) = (row.name.clone(), row.label.clone());
                if field.typ.is_nullable() && !matches!(value, Some(Value::Object(_))) {
                    let enabled = schema.new_object_value(&object, None);
                    self.model.set(&name, Value::Object(enabled));
                }
                return Step::Open(Frame::object(
                    schema,
                    can_get,
                    &object,
                    &name,
                    &label,
                    self.model.value(&name),
                    self.model.rules,
                ));
            }
            Opener::Expression => {
                return Step::Open(Frame::expression(
                    &row.name,
                    &row.label,
                    value,
                    self.model.error(&row.name),
                    self.model.rules,
                ));
            }
            Opener::List { object, items } => {
                return Step::Open(Frame::list(
                    &object,
                    &row.name,
                    &row.label,
                    value,
                    items,
                    self.model.rules,
                ));
            }
            Opener::Map => {
                if let Some(shape) = map_shape(&field.typ, row) {
                    return Step::Open(Frame::map(
                        &row.name,
                        &row.label,
                        &shape,
                        value,
                        self.model.rules,
                    ));
                }
            }
            Opener::Json | Opener::Unsupported => {
                let text = value
                    .and_then(|v| serde_json::to_string_pretty(v).ok())
                    .unwrap_or_default();
                self.editing = Some(Editing::Json(TextArea::new(&text)));
            }
        }
        Step::Event(FormEvent::None)
    }

    fn view_row(
        &mut self,
        schema: &Schema,
        can_get: &dyn Fn(&str) -> bool,
        typ: &FieldType,
    ) -> Step {
        let Some(row) = self.model.layout.rows.get(self.row) else {
            return Step::Event(FormEvent::None);
        };
        let mut rules = self.model.rules;
        rules.read_only = true;
        let value = match self.model.value(&row.name) {
            None | Some(Value::Null) => return Step::Event(FormEvent::None),
            Some(value) => value,
        };
        match (opener(schema, typ), value) {
            (Opener::Object(object), Value::Object(_)) => Step::Open(Frame::object(
                schema,
                can_get,
                &object,
                &row.name,
                &row.label,
                Some(value),
                rules,
            )),
            (Opener::Expression, Value::Object(_)) => Step::Open(Frame::expression(
                &row.name,
                &row.label,
                Some(value),
                None,
                rules,
            )),
            (Opener::List { object, items }, Value::Object(_) | Value::Array(_)) => Step::Open(
                Frame::list(&object, &row.name, &row.label, Some(value), items, rules),
            ),
            (Opener::Map, Value::Object(_)) => match map_shape(typ, row) {
                Some(shape) => Step::Open(Frame::map(
                    &row.name,
                    &row.label,
                    &shape,
                    Some(value),
                    rules,
                )),
                None => Step::Event(FormEvent::None),
            },
            (_, Value::String(text)) => {
                self.editing = Some(Editing::View(TextArea::new(text).at_top()));
                Step::Event(FormEvent::None)
            }
            (_, value) => {
                let text = serde_json::to_string_pretty(value).unwrap_or_default();
                self.editing = Some(Editing::View(TextArea::new(&text).at_top()));
                Step::Event(FormEvent::None)
            }
        }
    }

    pub fn edit_key(
        &mut self,
        schema: &Schema,
        can_get: &dyn Fn(&str) -> bool,
        event: KeyEvent,
        action: Option<EditAction>,
    ) {
        match self.stack.last_mut() {
            Some(Frame::Map { map, .. }) => {
                let step = map.edit_key(schema, can_get, event, action);
                self.map_step(step);
                return;
            }
            Some(Frame::Expression { expression, .. }) => {
                expression.edit_key(event, action);
                return;
            }
            _ => {}
        }
        if !self.stack.is_empty() {
            if let Some(view) = self.active_mut() {
                view.edit_key(schema, can_get, event, action);
            }
            return;
        }
        let multiline = match &mut self.editing {
            Some(Editing::Inline(inline)) => inline.is_multiline(),
            Some(Editing::Lines(_) | Editing::Json(_)) => true,
            Some(Editing::View(area)) => {
                match action {
                    Some(EditAction::Commit | EditAction::Done | EditAction::Cancel) => {
                        self.editing = None
                    }
                    Some(EditAction::Copy) => {}
                    None if is_navigation(event.code) => {
                        area.handle(event);
                    }
                    None => {}
                }
                return;
            }
            Some(Editing::Otp(_)) => false,
            None => return,
        };
        match action {
            Some(EditAction::Copy) => {}
            Some(EditAction::Cancel) => {
                self.editing = None;
                if std::mem::take(&mut self.rejected)
                    && let Some(name) = self.editable_name()
                {
                    self.model.clear_error(&name);
                }
            }
            Some(EditAction::Done) => self.commit(schema),
            Some(EditAction::Commit) if !multiline => self.commit(schema),
            Some(EditAction::Commit) | None => match &mut self.editing {
                Some(Editing::Inline(inline)) => {
                    inline.handle(event);
                }
                Some(Editing::Lines(area) | Editing::Json(area)) => {
                    area.handle(event);
                }
                Some(Editing::View(_)) => {}
                Some(Editing::Otp(editor)) => {
                    if let Some(name) = self.model.layout.rows.get(self.row).map(|r| r.name.clone())
                        && let Some(value) = editor.key(event, self.model.value(&name))
                    {
                        self.editing = None;
                        self.model.set(&name, value);
                    }
                }
                None => {}
            },
        }
    }

    pub fn paste(&mut self, text: &str) {
        match self.stack.last_mut() {
            Some(Frame::Map { map, .. }) => {
                map.paste(text);
                return;
            }
            Some(Frame::Expression { expression, .. }) => {
                expression.paste(text);
                return;
            }
            _ => {}
        }
        if !self.stack.is_empty() {
            if let Some(view) = self.active_mut() {
                view.paste(text);
            }
            return;
        }
        match &mut self.editing {
            Some(Editing::Inline(inline)) => inline.paste(text),
            Some(Editing::Lines(area) | Editing::Json(area)) => area.paste(text),
            Some(Editing::Otp(editor)) => editor.paste(text),
            Some(Editing::View(_)) | None => {}
        }
    }

    fn commit(&mut self, schema: &Schema) {
        let Some(name) = self.editable_name() else {
            self.editing = None;
            return;
        };
        if let Some(Editing::Otp(editor)) = &mut self.editing {
            if let Some(value) = editor.commit(self.model.value(&name)) {
                self.editing = None;
                self.totp = None;
                self.model.set(&name, value);
            }
            return;
        }
        let result = match &self.editing {
            Some(Editing::Inline(inline)) => inline.commit(),
            Some(Editing::Lines(area)) => Ok(lines_to_set(&area.text())),
            Some(Editing::Json(area)) => {
                let typ = self.model.field(schema, &name).map(|f| &f.typ);
                parse_json(&area.text(), typ)
            }
            Some(Editing::View(_)) => {
                self.editing = None;
                return;
            }
            Some(Editing::Otp(_)) | None => return,
        };
        self.rejected = result.is_err();
        match result {
            Ok(value) => {
                self.editing = None;
                self.model.set(&name, value);
            }
            Err(message) => self.model.set_error(&name, message),
        }
    }

    pub fn apply_pick(
        &mut self,
        schema: &Schema,
        can_get: &dyn Fn(&str) -> bool,
        pick: &Pick,
        keys: &[Box<str>],
    ) {
        if let (PickAt::MapKey | PickAt::MapValue(_), Some(Frame::Map { map, .. })) =
            (pick.at, self.stack.last_mut())
        {
            if let Some(key) = keys.first() {
                let step = map.apply_pick(schema, can_get, pick.at, key);
                self.map_step(step);
            }
            return;
        }
        if !self.stack.is_empty() {
            if let Some(view) = self.active_mut() {
                view.apply_pick(schema, can_get, pick, keys);
            }
            return;
        }
        let PickAt::Row(row) = pick.at else {
            return;
        };
        let Some(row) = self.model.layout.rows.get(row) else {
            return;
        };
        let name = row.name.clone();
        let value = match &pick.kind {
            PickKind::Variant => {
                let Some(key) = keys.first() else {
                    return;
                };
                let object = self.model.shape.schema_key(schema).to_string();
                let Some(variant) = self
                    .model
                    .shape
                    .variants(schema)
                    .iter()
                    .find(|v| *v.name == **key)
                else {
                    return;
                };
                let variants = self.model.shape.variants(schema);
                let values = &self.model.values;
                let original = &self.model.original;
                let next = schema.switch_variant(values, &object, variant);
                self.model.values = if original.get(AT_TYPE).and_then(Value::as_str)
                    == Some(variant.name.as_str())
                {
                    let mut restored = original.clone();
                    let current = values.get(AT_TYPE).and_then(Value::as_str);
                    if let Some(current) =
                        variants.iter().find(|v| Some(v.name.as_str()) == current)
                    {
                        let baseline = schema.switch_variant(original, &object, current);
                        for (key, value) in next {
                            if values.get(&key) == Some(&value)
                                && baseline.get(&key) != Some(&value)
                            {
                                restored.insert(key, value);
                            }
                        }
                    }
                    restored
                } else {
                    next
                };
                self.model.clear_errors();
                self.model.relayout(schema, can_get);
                return;
            }
            PickKind::Enum { multi: true, .. } | PickKind::Reference { multi: true, .. } => {
                Value::Object(
                    keys.iter()
                        .map(|k| (k.to_string(), Value::Bool(true)))
                        .collect::<Map<_, _>>(),
                )
            }
            PickKind::Enum { .. } | PickKind::Reference { .. } => match keys.first() {
                Some(key) if !key.is_empty() => Value::String(key.to_string()),
                _ => Value::Null,
            },
        };
        self.model.set(&name, value);
    }

    pub fn inline_lines<'a>(
        &'a mut self,
        theme: &crate::tui::theme::Theme,
        width: u16,
        height: usize,
    ) -> Vec<ratatui::text::Line<'a>> {
        match &mut self.editing {
            Some(Editing::Inline(inline)) => inline.lines(theme, width, height),
            Some(Editing::Lines(area) | Editing::Json(area) | Editing::View(area)) => {
                area.lines(theme, height, true).collect()
            }
            Some(Editing::Otp(editor)) => editor.lines(theme, width),
            None => Vec::new(),
        }
    }
}

fn opener(schema: &Schema, typ: &FieldType) -> Opener {
    if let Some(kind) = Kind::of_field(typ) {
        return Opener::Inline(kind);
    }
    match typ {
        FieldType::Boolean => Opener::Toggle,
        FieldType::Enum {
            enum_name,
            nullable,
        } => Opener::Pick(PickKind::Enum {
            name: enum_name.as_str().into(),
            nullable: *nullable,
            multi: false,
        }),
        FieldType::ObjectId {
            object_name,
            nullable,
        } => Opener::Pick(PickKind::Reference {
            object: object_name.as_str().into(),
            nullable: *nullable,
            multi: false,
        }),
        FieldType::Set { class, .. } => match class {
            ScalarType::Enum { enum_name } => Opener::Pick(PickKind::Enum {
                name: enum_name.as_str().into(),
                nullable: false,
                multi: true,
            }),
            ScalarType::ObjectId { object_name } => Opener::Pick(PickKind::Reference {
                object: object_name.as_str().into(),
                nullable: false,
                multi: true,
            }),
            ScalarType::String { .. } => Opener::Lines,
        },
        FieldType::Object { object_name, .. } if object_name == OTP_AUTH => Opener::Otp,
        FieldType::Object { object_name, .. } if schema.is_expression(object_name) => {
            Opener::Expression
        }
        FieldType::Object { object_name, .. } if object_name == PASSKEY_REGISTRATION => {
            Opener::Passkey
        }
        FieldType::Object { object_name, .. } if schema.schemas.contains_key(object_name) => {
            Opener::Object(object_name.as_str().into())
        }
        FieldType::ObjectList {
            object_name,
            min_items,
            max_items,
        } if schema.schemas.contains_key(object_name) => Opener::List {
            object: object_name.as_str().into(),
            items: (*min_items, *max_items),
        },
        FieldType::Map { .. } => Opener::Map,
        FieldType::Object { .. } | FieldType::ObjectList { .. } => Opener::Json,
        _ => Opener::Unsupported,
    }
}

fn map_shape<'a>(typ: &'a FieldType, row: &'a Row) -> Option<MapShape<'a>> {
    let FieldType::Map {
        key_class,
        value_class,
        min_items,
        max_items,
    } = typ
    else {
        return None;
    };
    Some(MapShape {
        key_class,
        value_class,
        items: (*min_items, *max_items),
        key_label: row.key_label.as_deref(),
        value_label: row.value_label.as_deref(),
    })
}

fn is_navigation(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End
    )
}

fn lines_to_set(text: &str) -> Value {
    Value::Object(
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| (line.to_string(), Value::Bool(true)))
            .collect(),
    )
}

fn parse_json(text: &str, typ: Option<&FieldType>) -> Result<Value, String> {
    if text.trim().is_empty() && typ.is_some_and(FieldType::is_nullable) {
        return Ok(Value::Null);
    }
    let value: Value = serde_json::from_str(text).map_err(|e| format!("Invalid JSON: {e}"))?;
    match (typ, &value) {
        (
            Some(FieldType::Object { .. } | FieldType::Map { .. } | FieldType::ObjectList { .. }),
            Value::Object(_),
        )
        | (Some(FieldType::Object { nullable: true, .. }), Value::Null)
        | (None, _) => Ok(value),
        (
            Some(FieldType::Object { .. } | FieldType::Map { .. } | FieldType::ObjectList { .. }),
            _,
        ) => Err("Expected a JSON object".into()),
        (Some(_), _) => Ok(value),
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/form/view/tests.rs"]
mod tests;
