/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::account::{Op, Permissions};
use crate::jmap::set_error::{SetError, SetErrorType};
use crate::schema::expression::{EMPTY, Expression};
use crate::schema::object::FieldSet;
use crate::schema::view::{FieldEntry, FormSections};
use crate::schema::{
    AT_TYPE, Field, FieldType, FieldUpdate, Fields, MapValueType, ObjectSchema, ObjectType,
    ObjectVariant, ScalarType, Schema, StringFormat,
};
use serde_json::{Map, Number, Value};
use std::fmt::Write;
use std::ops::Range;

pub use crate::input::patch::SECRET_MASK;
pub const PASSKEY_REGISTRATION: &str = "x:PasskeyRegistration";
pub const OTP_URL: &str = "otpUrl";
pub const ALREADY_IN_USE: &str = "Already in use by another object; choose a different value";
pub const OTP_CODE: &str = "otpCode";
pub const MAX_NESTING: usize = 16;
pub const CHANGED_SECRET: &str = "•••••• (changed)";
const SHOWN_SECRET: &str = "••••••";
const OTP_PENDING: &str = "Enter the code from your authenticator app to confirm two-factor setup";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Create,
    Edit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnterpriseFields {
    Editable,
    Locked,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Edit,
    ReadOnly,
    Locked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    Required,
    Optional,
    Plain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Field,
    Variant,
}

#[derive(Clone, Copy, Debug)]
pub struct Rules {
    pub mode: Mode,
    pub enterprise: EnterpriseFields,
    pub read_only: bool,
    pub mutable_only: bool,
}

impl Rules {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            enterprise: EnterpriseFields::Editable,
            read_only: false,
            mutable_only: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub name: Box<str>,
    pub label: Box<str>,
    pub kind: RowKind,
    pub access: Access,
    pub marker: Marker,
    pub placeholder: Option<Box<str>>,
    pub key_label: Option<Box<str>>,
    pub value_label: Option<Box<str>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: Option<Box<str>>,
    pub rows: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    View(Box<str>),
    Embedded(Box<str>),
}

impl Shape {
    pub fn schema_key<'s>(&'s self, schema: &'s Schema) -> &'s str {
        match self {
            Shape::View(view) => schema.view(view).map_or(view, |def| def.object.name()),
            Shape::Embedded(name) => name,
        }
    }

    pub fn object_schema<'s>(&self, schema: &'s Schema) -> Option<&'s ObjectSchema> {
        schema.schemas.get(self.schema_key(schema))
    }

    pub fn variants<'s>(&self, schema: &'s Schema) -> &'s [ObjectVariant] {
        match self.object_schema(schema) {
            Some(ObjectSchema::Multiple { variants }) => variants,
            _ => &[],
        }
    }

    pub fn fields<'s>(
        &self,
        schema: &'s Schema,
        values: &Map<String, Value>,
    ) -> Option<&'s Fields> {
        shape_fields(schema, self.schema_key(schema), values)
    }

    pub fn sections<'s>(&self, schema: &'s Schema, at_type: Option<&str>) -> FormSections<'s> {
        match self {
            Shape::View(view) => schema
                .view(view)
                .map(|def| def.form_sections(at_type))
                .unwrap_or_default(),
            Shape::Embedded(name) => schema.embedded_form_sections(name, at_type),
        }
    }
}

pub fn shape_fields<'s>(
    schema: &'s Schema,
    key: &str,
    values: &Map<String, Value>,
) -> Option<&'s Fields> {
    match schema.schemas.get(key)? {
        ObjectSchema::Single { schema_name } => schema.fields.get(schema_name),
        shape @ ObjectSchema::Multiple { .. } => {
            let name = shape.variant_of(values)?.schema_name.as_deref()?;
            schema.fields.get(name)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub title: Option<Box<str>>,
    pub sections: Vec<Section>,
    pub rows: Vec<Row>,
}

impl Layout {
    pub fn build(
        schema: &Schema,
        shape: &Shape,
        values: &Map<String, Value>,
        rules: Rules,
        can_get: &dyn Fn(&str) -> bool,
    ) -> Self {
        let at_type = values.get(AT_TYPE).and_then(Value::as_str);
        let multi = !shape.variants(schema).is_empty();
        let declared = shape.sections(schema, at_type);
        let mut layout = Layout {
            title: declared.title.map(Box::from),
            ..Layout::default()
        };
        let mut has_variant_row = false;
        for section in &declared.sections {
            let start = layout.rows.len();
            for entry in &section.fields {
                match entry.field {
                    None if entry.name == AT_TYPE => {
                        if multi && !has_variant_row {
                            has_variant_row = true;
                            layout.rows.push(Row::variant(entry.label, rules));
                        }
                    }
                    None => {}
                    Some(field) => {
                        if let Some(row) = Row::field(schema, entry, field, rules, can_get) {
                            layout.rows.push(row);
                        }
                    }
                }
            }
            if layout.rows.len() > start {
                layout.sections.push(Section {
                    title: section.title.map(Box::from),
                    rows: start..layout.rows.len(),
                });
            }
        }
        if multi && !has_variant_row {
            layout.rows.insert(0, Row::variant("Type", rules));
            for section in &mut layout.sections {
                section.rows = section.rows.start + 1..section.rows.end + 1;
            }
            match layout.sections.first_mut() {
                Some(first) => first.rows.start = 0,
                None => layout.sections.push(Section {
                    title: None,
                    rows: 0..1,
                }),
            }
        }
        layout
    }

    pub fn row(&self, name: &str) -> Option<usize> {
        self.rows.iter().position(|row| &*row.name == name)
    }

    pub fn section_of(&self, row: usize) -> Option<usize> {
        self.sections.iter().position(|s| s.rows.contains(&row))
    }
}

impl Row {
    fn variant(label: &str, rules: Rules) -> Self {
        let access = if rules.read_only || (rules.mutable_only && rules.mode == Mode::Edit) {
            Access::ReadOnly
        } else {
            Access::Edit
        };
        Self {
            name: AT_TYPE.into(),
            label: Box::from(if label.is_empty() || label == AT_TYPE {
                "Type"
            } else {
                label
            }),
            kind: RowKind::Variant,
            access,
            marker: Marker::Plain,
            placeholder: None,
            key_label: None,
            value_label: None,
        }
    }

    fn field(
        schema: &Schema,
        entry: &FieldEntry<'_>,
        field: &Field,
        rules: Rules,
        can_get: &dyn Fn(&str) -> bool,
    ) -> Option<Self> {
        let server_set = field.update == FieldUpdate::ServerSet;
        match rules.mode {
            Mode::Create if server_set => return None,
            Mode::Edit if is_create_only(&field.typ) => return None,
            _ => {}
        }
        if rules.mutable_only && field.update != FieldUpdate::Mutable {
            return None;
        }
        if field.enterprise && rules.enterprise == EnterpriseFields::Hidden {
            return None;
        }
        if referenced_objects(&field.typ)
            .filter_map(|object| permission_object(schema, object))
            .any(|object| !can_get(object))
        {
            return None;
        }
        let locked = rules.enterprise == EnterpriseFields::Locked
            && (field.enterprise || references_enterprise(schema, &field.typ));
        let access = if locked {
            Access::Locked
        } else if rules.read_only
            || server_set
            || (rules.mode == Mode::Edit && field.update == FieldUpdate::Immutable)
        {
            Access::ReadOnly
        } else {
            Access::Edit
        };
        let marker = if access == Access::ReadOnly || !is_required_eligible(&field.typ) {
            Marker::Plain
        } else if field.typ.is_nullable() {
            Marker::Optional
        } else {
            Marker::Required
        };
        Some(Self {
            name: entry.name.into(),
            label: entry.label.into(),
            kind: RowKind::Field,
            access,
            marker,
            placeholder: entry
                .form_field
                .and_then(|f| f.placeholder.as_deref())
                .map(Box::from),
            key_label: entry
                .form_field
                .and_then(|f| f.key_label.as_deref())
                .map(Box::from),
            value_label: entry
                .form_field
                .and_then(|f| f.value_label.as_deref())
                .map(Box::from),
        })
    }
}

fn references_enterprise(schema: &Schema, typ: &FieldType) -> bool {
    match typ {
        FieldType::ObjectId { object_name, .. } => permission_object(schema, object_name)
            .and_then(|object| schema.get_object(object))
            .is_some_and(|def| def.is_enterprise()),
        _ => false,
    }
}

fn is_create_only(typ: &FieldType) -> bool {
    matches!(typ, FieldType::Object { object_name, .. } if object_name == PASSKEY_REGISTRATION)
}

pub fn permission_object<'s>(schema: &'s Schema, name: &'s str) -> Option<&'s str> {
    match schema.objects.get(name)? {
        ObjectType::View { object_name } => Some(object_name),
        ObjectType::Object { .. } | ObjectType::Singleton { .. } => Some(name),
    }
}

pub fn allowed(schema: &Schema, permissions: &Permissions, name: &str, op: Op) -> bool {
    permission_object(schema, name)
        .and_then(|object| schema.get_object(object))
        .and_then(|def| def.permission_prefix())
        .is_none_or(|prefix| prefix.is_empty() || permissions.can(prefix, op))
}

pub fn is_required_eligible(typ: &FieldType) -> bool {
    matches!(
        typ,
        FieldType::String { .. }
            | FieldType::Number { .. }
            | FieldType::UtcDateTime { .. }
            | FieldType::Enum { .. }
            | FieldType::BlobId
            | FieldType::ObjectId { .. }
    )
}

pub fn is_required(field: &Field, mode: Mode) -> bool {
    is_required_eligible(&field.typ)
        && !field.typ.is_nullable()
        && field.update != FieldUpdate::ServerSet
        && !(mode == Mode::Edit && field.update == FieldUpdate::Immutable)
}

pub fn is_missing(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        Some(_) => false,
    }
}

pub fn referenced_objects(typ: &FieldType) -> impl Iterator<Item = &str> {
    let (first, second) = match typ {
        FieldType::ObjectId { object_name, .. }
        | FieldType::Object { object_name, .. }
        | FieldType::ObjectList { object_name, .. } => (Some(object_name.as_str()), None),
        FieldType::Set {
            class: ScalarType::ObjectId { object_name },
            ..
        } => (Some(object_name.as_str()), None),
        FieldType::Map {
            key_class,
            value_class,
            ..
        } => (
            match key_class {
                ScalarType::ObjectId { object_name } => Some(object_name.as_str()),
                _ => None,
            },
            match value_class {
                MapValueType::Object { object_name } => Some(object_name.as_str()),
                _ => None,
            },
        ),
        _ => (None, None),
    };
    first.into_iter().chain(second)
}

#[derive(Clone, Debug)]
pub struct FormModel {
    pub shape: Shape,
    pub rules: Rules,
    pub original: Map<String, Value>,
    pub values: Map<String, Value>,
    pub layout: Layout,
    errors: Vec<(Box<str>, Box<str>)>,
    pub banner: Option<Box<str>>,
}

impl FormModel {
    pub fn new(
        schema: &Schema,
        shape: Shape,
        rules: Rules,
        original: Map<String, Value>,
        values: Map<String, Value>,
        can_get: &dyn Fn(&str) -> bool,
    ) -> Self {
        let layout = Layout::build(schema, &shape, &values, rules, can_get);
        Self {
            shape,
            rules,
            original,
            values,
            layout,
            errors: Vec::new(),
            banner: None,
        }
    }

    pub fn relayout(&mut self, schema: &Schema, can_get: &dyn Fn(&str) -> bool) {
        self.layout = Layout::build(schema, &self.shape, &self.values, self.rules, can_get);
    }

    pub fn fields<'s>(&self, schema: &'s Schema) -> Option<&'s Fields> {
        self.shape.fields(schema, &self.values)
    }

    pub fn field<'s>(&self, schema: &'s Schema, name: &str) -> Option<&'s Field> {
        self.fields(schema)?.properties.get(name)
    }

    pub fn at_type(&self) -> Option<&str> {
        self.values.get(AT_TYPE).and_then(Value::as_str)
    }

    pub fn value(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }

    pub fn set(&mut self, name: &str, value: Value) {
        self.clear_error(name);
        self.values.insert(name.into(), value);
    }

    pub fn revert(&mut self, name: &str) {
        self.clear_error(name);
        match self.original.get(name) {
            Some(value) => {
                self.values.insert(name.into(), value.clone());
            }
            None => {
                self.values.shift_remove(name);
            }
        }
        if !self.has_changes() {
            self.clear_errors();
        }
    }

    pub fn is_dirty(&self, name: &str) -> bool {
        self.values.get(name) != self.original.get(name)
    }

    pub fn dirty_count(&self) -> usize {
        self.layout
            .rows
            .iter()
            .filter(|row| self.is_dirty(&row.name))
            .count()
    }

    pub fn has_changes(&self) -> bool {
        self.values.len() != self.original.len()
            || self
                .values
                .iter()
                .any(|(key, value)| self.original.get(key) != Some(value))
    }

    pub fn section_dirty(&self, section: &Section) -> bool {
        self.layout
            .rows
            .get(section.rows.clone())
            .is_some_and(|rows| rows.iter().any(|row| self.is_dirty(&row.name)))
    }

    pub fn section_has_error(&self, section: &Section) -> bool {
        self.layout
            .rows
            .get(section.rows.clone())
            .is_some_and(|rows| rows.iter().any(|row| self.error(&row.name).is_some()))
    }

    pub fn error(&self, name: &str) -> Option<&str> {
        self.errors
            .iter()
            .find(|(field, _)| &**field == name)
            .map(|(_, message)| &**message)
    }

    pub fn error_count(&self) -> usize {
        self.errors.len()
    }

    pub fn set_error(&mut self, name: &str, message: impl Into<Box<str>>) {
        let message = message.into();
        match self.errors.iter_mut().find(|(field, _)| &**field == name) {
            Some((_, existing)) => *existing = message,
            None => self.errors.push((name.into(), message)),
        }
    }

    pub fn clear_error(&mut self, name: &str) {
        self.errors.retain(|(field, _)| &**field != name);
    }

    pub fn clear_errors(&mut self) {
        self.errors.clear();
        self.banner = None;
    }

    pub fn validate(&mut self, schema: &Schema) -> bool {
        self.errors.clear();
        let mode = self.rules.mode;
        let mut message = String::new();
        let mut otp_pending = false;
        let Some(fields) = self.fields(schema) else {
            return true;
        };
        for row in &self.layout.rows {
            if row.access != Access::Edit || row.kind == RowKind::Variant {
                continue;
            }
            let Some(field) = fields.properties.get(&*row.name) else {
                continue;
            };
            let value = self.values.get(&*row.name);
            message.clear();
            if is_required(field, mode) && is_missing(value) {
                message.push_str("This field is required");
            } else if let Some(value) = value {
                check_value(schema, field, value, &mut message, 0);
            }
            if message.is_empty()
                && let Some(Value::Object(map)) = value
                && !otp_valid(map, 0)
            {
                message.push_str(OTP_PENDING);
                otp_pending = true;
            }
            if !message.is_empty() {
                self.errors
                    .push((row.name.clone(), message.as_str().into()));
            }
        }
        if otp_pending || !otp_valid(&self.values, 0) {
            self.banner = Some(OTP_PENDING.into());
            return false;
        }
        self.errors.is_empty()
    }

    pub fn apply_set_error(&mut self, err: &SetError, fallback: &str) {
        let known = |name: &str| self.layout.row(name).is_some();
        let mut field_errors: Vec<(Box<str>, Box<str>)> = Vec::new();
        let mut banner = String::from(fallback);
        for property in err.properties.iter().flatten() {
            let top = property.split('/').next().unwrap_or(property);
            if known(top) {
                let fallback = match err.type_ {
                    SetErrorType::PrimaryKeyViolation => ALREADY_IN_USE,
                    _ => "Invalid value",
                };
                let text = err
                    .description
                    .as_deref()
                    .filter(|d| !d.is_empty())
                    .unwrap_or(fallback);
                field_errors.push((top.into(), text.into()));
            }
        }
        let mut text = String::new();
        for validation in &err.validation_errors {
            let Some(property) = validation.property() else {
                continue;
            };
            let (top, rest) = property.split_once('/').unwrap_or((property, ""));
            text.clear();
            if !rest.is_empty() {
                let _ = write!(text, "{rest}: ");
            }
            let _ = validation.message(&mut text);
            if known(top) {
                field_errors.push((top.into(), text.as_str().into()));
            } else {
                let _ = write!(banner, "\n{property}: {text}");
            }
        }
        for (field, message) in field_errors {
            self.set_error(&field, message);
        }
        self.banner = Some(banner.into_boxed_str());
    }

    pub fn first_error_row(&self) -> Option<usize> {
        self.layout
            .rows
            .iter()
            .position(|row| self.error(&row.name).is_some())
    }

    pub fn first_input_row(&self) -> Option<usize> {
        let rows = &self.layout.rows;
        let editable = |row: &Row| row.access == Access::Edit && row.kind == RowKind::Field;
        rows.iter()
            .position(|row| {
                editable(row) && row.marker == Marker::Required && is_missing(self.value(&row.name))
            })
            .or_else(|| rows.iter().position(editable))
    }
}

fn check_value(schema: &Schema, field: &Field, value: &Value, out: &mut String, depth: usize) {
    match (&field.typ, value) {
        (
            FieldType::String {
                min_length,
                max_length,
                ..
            },
            Value::String(s),
        ) => {
            if field.typ.is_secret_scalar() && s == SECRET_MASK {
                return;
            }
            check_length(s, *min_length, *max_length, out);
        }
        (FieldType::Number { min, max, .. }, Value::Number(n)) => {
            check_range(n, min.as_ref(), max.as_ref(), out)
        }
        (
            FieldType::Set {
                min_items,
                max_items,
                ..
            }
            | FieldType::ObjectList {
                min_items,
                max_items,
                ..
            }
            | FieldType::Map {
                min_items,
                max_items,
                ..
            },
            Value::Object(items),
        ) => {
            let len = items.len();
            if let Some(min) = min_items.filter(|min| len < *min) {
                let _ = write!(out, "Needs at least {min} item{}", plural(min));
            } else if let Some(max) = max_items.filter(|max| len > *max) {
                let _ = write!(out, "Allows at most {max} item{}", plural(max));
            } else if let FieldType::Set {
                class:
                    ScalarType::String {
                        min_length,
                        max_length,
                        ..
                    },
                ..
            } = &field.typ
            {
                for member in items.keys() {
                    let start = out.len();
                    let _ = write!(out, "{member} › ");
                    let mark = out.len();
                    check_length(member, *min_length, *max_length, out);
                    if out.len() > mark {
                        return;
                    }
                    out.truncate(start);
                }
            } else if let FieldType::Map {
                value_class:
                    value_class @ (MapValueType::String { .. } | MapValueType::Number { .. }),
                ..
            } = &field.typ
            {
                for (key, item) in items {
                    let start = out.len();
                    let _ = write!(out, "{key} › ");
                    let mark = out.len();
                    match (value_class, item) {
                        (
                            MapValueType::String {
                                format,
                                min_length,
                                max_length,
                            },
                            Value::String(s),
                        ) if !(matches!(
                            format,
                            StringFormat::Secret | StringFormat::SecretText
                        ) && s == SECRET_MASK) =>
                        {
                            check_length(s, *min_length, *max_length, out)
                        }
                        (MapValueType::Number { min, max, .. }, Value::Number(n)) => {
                            check_range(n, min.as_ref(), max.as_ref(), out)
                        }
                        _ => {}
                    }
                    if out.len() > mark {
                        return;
                    }
                    out.truncate(start);
                }
            } else if let FieldType::ObjectList { object_name, .. } = &field.typ
                && depth < MAX_NESTING
            {
                for (index, item) in items.values().enumerate() {
                    let Value::Object(item) = item else {
                        continue;
                    };
                    let start = out.len();
                    let _ = write!(out, "#{} › ", index + 1);
                    let mark = out.len();
                    check_nested(schema, object_name, item, depth + 1, out);
                    if out.len() > mark {
                        return;
                    }
                    out.truncate(start);
                }
            } else if let FieldType::Map {
                value_class: MapValueType::Object { object_name },
                ..
            } = &field.typ
                && depth < MAX_NESTING
            {
                for (key, item) in items {
                    let Value::Object(item) = item else {
                        continue;
                    };
                    let start = out.len();
                    let _ = write!(out, "{key} › ");
                    let mark = out.len();
                    check_nested(schema, object_name, item, depth + 1, out);
                    if out.len() > mark {
                        return;
                    }
                    out.truncate(start);
                }
            }
        }
        (FieldType::Object { object_name, .. }, Value::Object(_))
            if schema.is_expression(object_name) =>
        {
            if let Some(part) =
                Expression::parse(value).and_then(|expression| expression.empty_part())
            {
                part.describe(EMPTY, out);
            }
        }
        (FieldType::Object { object_name, .. }, Value::Object(obj)) if depth < MAX_NESTING => {
            check_nested(schema, object_name, obj, depth + 1, out);
        }
        _ => {}
    }
}

fn check_length(s: &str, min_length: Option<usize>, max_length: Option<usize>, out: &mut String) {
    let len = s.chars().count();
    if let Some(min) = min_length.filter(|min| len < *min) {
        let _ = write!(out, "Must be at least {min} characters");
    } else if let Some(max) = max_length.filter(|max| len > *max) {
        let _ = write!(out, "Must be at most {max} characters");
    }
}

fn check_range(n: &Number, min: Option<&Number>, max: Option<&Number>, out: &mut String) {
    let v = n.as_f64().unwrap_or_default();
    if let Some(min) = min.filter(|min| v < min.as_f64().unwrap_or(f64::MIN)) {
        let _ = write!(out, "Must be at least {min}");
    } else if let Some(max) = max.filter(|max| v > max.as_f64().unwrap_or(f64::MAX)) {
        let _ = write!(out, "Must be at most {max}");
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn check_nested(
    schema: &Schema,
    object_name: &str,
    value: &Map<String, Value>,
    depth: usize,
    out: &mut String,
) {
    if depth > MAX_NESTING {
        return;
    }
    let Some(fields) = shape_fields(schema, object_name, value) else {
        return;
    };
    for (name, field) in &fields.properties {
        if field.update == FieldUpdate::ServerSet {
            continue;
        }
        let nested = value.get(name);
        let start = out.len();
        let _ = write!(out, "{name}: ");
        let mark = out.len();
        if is_required(field, Mode::Create) && is_missing(nested) {
            out.push_str("This field is required");
        } else if let Some(nested) = nested {
            check_value(schema, field, nested, out, depth);
        }
        if out.len() > mark {
            return;
        }
        out.truncate(start);
    }
}

pub fn item_summary<'v>(
    schema: &Schema,
    object: &str,
    item: &'v Map<String, Value>,
) -> Option<&'v str> {
    shape_fields(schema, object, item)?
        .properties
        .iter()
        .filter(|(name, field)| {
            name.as_str() != OTP_URL
                && !field.is_server_set()
                && !field.typ.is_secret_scalar()
                && matches!(field.typ, FieldType::String { .. })
        })
        .find_map(|(name, _)| item.get(name)?.as_str().filter(|text| !text.is_empty()))
}

pub fn is_changed_secret(typ: &FieldType, value: &Value) -> bool {
    typ.is_secret_scalar() && matches!(value, Value::String(s) if is_plain_secret(s))
}

fn is_plain_secret(text: &str) -> bool {
    !text.is_empty() && text != SECRET_MASK
}

pub fn mask_secrets(schema: &Schema, fields: FieldSet<'_>, values: &mut Map<String, Value>) {
    for (name, value) in values.iter_mut() {
        if let Some(field) = fields.get(name) {
            mask_value(schema, &field.typ, value, 0);
        }
    }
}

fn mask_value(schema: &Schema, typ: &FieldType, value: &mut Value, depth: usize) {
    if depth > MAX_NESTING {
        return;
    }
    match (typ, value) {
        (FieldType::String { .. }, Value::String(text)) if typ.is_secret_scalar() => {
            mask_text(text)
        }
        (FieldType::Object { object_name, .. }, Value::Object(map)) => {
            mask_nested(schema, object_name, map, depth + 1)
        }
        (FieldType::ObjectList { object_name, .. }, Value::Object(items)) => {
            for item in items.values_mut() {
                if let Value::Object(map) = item {
                    mask_nested(schema, object_name, map, depth + 1);
                }
            }
        }
        (FieldType::Map { value_class, .. }, Value::Object(items)) => match value_class {
            MapValueType::String {
                format: StringFormat::Secret | StringFormat::SecretText,
                ..
            } => {
                for item in items.values_mut() {
                    if let Value::String(text) = item {
                        mask_text(text);
                    }
                }
            }
            MapValueType::Object { object_name } => {
                for item in items.values_mut() {
                    if let Value::Object(map) = item {
                        mask_nested(schema, object_name, map, depth + 1);
                    }
                }
            }
            MapValueType::String { .. }
            | MapValueType::Number { .. }
            | MapValueType::Enum { .. } => {}
        },
        _ => {}
    }
}

fn mask_nested(schema: &Schema, object_name: &str, map: &mut Map<String, Value>, depth: usize) {
    let Some(fields) = shape_fields(schema, object_name, map) else {
        return;
    };
    for (name, field) in &fields.properties {
        if let Some(value) = map.get_mut(name) {
            mask_value(schema, &field.typ, value, depth);
        }
    }
}

fn mask_text(text: &mut String) {
    if is_plain_secret(text) {
        text.clear();
        text.push_str(SHOWN_SECRET);
    }
}

pub fn otp_valid(map: &Map<String, Value>, depth: usize) -> bool {
    if depth < MAX_NESTING
        && !map.values().all(|nested| match nested {
            Value::Object(nested) => otp_valid(nested, depth + 1),
            _ => true,
        })
    {
        return false;
    }
    if !map.contains_key(OTP_URL) || !map.contains_key(OTP_CODE) {
        return true;
    }
    if is_missing(map.get(OTP_URL)) {
        return true;
    }
    !matches!(map.get(OTP_CODE), None | Some(Value::Null))
        && !matches!(map.get(OTP_CODE), Some(Value::String(code)) if code.is_empty() || code == SECRET_MASK)
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/form/model/tests.rs"]
mod tests;
