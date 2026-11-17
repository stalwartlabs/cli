/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::{Dashboard, Layout};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use std::collections::HashMap;

pub const AT_TYPE: &str = "@type";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Schema {
    pub objects: IndexMap<String, ObjectType>,
    pub schemas: HashMap<String, ObjectSchema>,
    pub fields: HashMap<String, Fields>,
    pub forms: HashMap<String, Form>,
    pub lists: HashMap<String, List>,
    pub enums: HashMap<String, Vec<EnumVariant>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layouts: Vec<Layout>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dashboards: Vec<Dashboard>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "type")]
pub enum ObjectType {
    #[serde(rename_all = "camelCase")]
    Singleton {
        description: String,
        permission_prefix: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        enterprise: bool,
    },
    #[serde(rename_all = "camelCase")]
    Object {
        description: String,
        permission_prefix: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        enterprise: bool,
    },
    #[serde(rename_all = "camelCase")]
    View { object_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
pub enum ObjectSchema {
    #[serde(rename_all = "camelCase")]
    Single { schema_name: String },
    #[serde(rename_all = "camelCase")]
    Multiple { variants: Vec<ObjectVariant> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectVariant {
    pub name: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct List {
    pub title: String,
    pub subtitle: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_property: Option<String>,

    pub singular_name: String,
    pub plural_name: String,

    pub columns: Vec<Column>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub filters_static: HashMap<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mass_actions: Vec<MassAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub item_actions: Vec<ItemAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "type")]
pub enum MassAction {
    #[serde(rename_all = "camelCase")]
    SetProperty {
        label: String,
        properties: HashMap<String, Value>,
    },
    #[serde(rename_all = "camelCase")]
    Delete {
        label: String,
    },
    Separator,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
pub enum ItemAction {
    #[serde(rename_all = "camelCase")]
    Delete {
        label: String,
    },
    #[serde(rename_all = "camelCase")]
    SetProperty {
        label: String,
        properties: HashMap<String, Value>,
    },
    #[serde(rename_all = "camelCase")]
    Query {
        label: String,
        object_name: String,
        field_name: String,
    },
    #[serde(rename_all = "camelCase")]
    View {
        label: String,
        object_name: String,
    },
    Separator,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
pub enum Filter {
    #[serde(rename_all = "camelCase")]
    Text { field: String, label: String },
    #[serde(rename_all = "camelCase")]
    Enum {
        field: String,
        enum_name: String,
        label: String,
    },
    #[serde(rename_all = "camelCase")]
    Integer { field: String, label: String },
    #[serde(rename_all = "camelCase")]
    Date { field: String, label: String },
    #[serde(rename_all = "camelCase")]
    ObjectId {
        field: String,
        object_name: String,
        label: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Form {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    pub sections: Vec<FormSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FormSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub fields: Vec<FormField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FormField {
    pub name: String,
    pub label: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_label: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Fields {
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub properties: IndexMap<String, Field>,

    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub defaults: HashMap<String, Value>,
}

impl Fields {
    pub fn comparable_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .properties
            .iter()
            .filter(|(_, f)| f.is_comparable_scalar())
            .map(|(n, _)| n.as_str())
            .collect();
        names.sort_unstable();
        names
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub description: String,
    #[serde(rename = "type")]
    pub typ: FieldType,
    pub update: FieldUpdate,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub enterprise: bool,
}

impl Field {
    pub fn is_server_set(&self) -> bool {
        matches!(self.update, FieldUpdate::ServerSet)
    }

    pub fn is_patchable(&self) -> bool {
        !matches!(self.update, FieldUpdate::Immutable | FieldUpdate::ServerSet)
    }

    pub fn is_object_id(&self) -> bool {
        matches!(self.typ, FieldType::ObjectId { .. })
    }

    pub fn is_comparable_scalar(&self) -> bool {
        !self.is_server_set()
            && !self.typ.is_secret_scalar()
            && matches!(
                self.typ,
                FieldType::String { .. }
                    | FieldType::Number { .. }
                    | FieldType::Boolean
                    | FieldType::Enum { .. }
                    | FieldType::UtcDateTime { .. }
            )
    }
}

impl FieldType {
    pub fn is_secret_scalar(&self) -> bool {
        matches!(
            self,
            FieldType::String {
                format: StringFormat::Secret | StringFormat::SecretText,
                ..
            }
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
pub enum FieldType {
    #[serde(rename_all = "camelCase")]
    String {
        format: StringFormat,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_length: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_length: Option<usize>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        nullable: bool,
    },
    #[serde(rename_all = "camelCase")]
    Number {
        format: NumberFormat,
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<Number>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<Number>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        nullable: bool,
    },
    #[serde(rename_all = "camelCase")]
    UtcDateTime {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        nullable: bool,
    },
    Boolean,
    #[serde(rename_all = "camelCase")]
    Enum {
        enum_name: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        nullable: bool,
    },
    BlobId,
    #[serde(rename_all = "camelCase")]
    ObjectId {
        object_name: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        nullable: bool,
    },
    #[serde(rename_all = "camelCase")]
    Object {
        object_name: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        nullable: bool,
    },
    #[serde(rename_all = "camelCase")]
    ObjectList {
        object_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_items: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_items: Option<usize>,
    },
    #[serde(rename_all = "camelCase")]
    Set {
        class: ScalarType,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_items: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_items: Option<usize>,
    },
    #[serde(rename_all = "camelCase")]
    Map {
        key_class: ScalarType,
        value_class: MapValueType,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_items: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_items: Option<usize>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
pub enum ScalarType {
    #[serde(rename_all = "camelCase")]
    String {
        format: StringFormat,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_length: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_length: Option<usize>,
    },
    #[serde(rename_all = "camelCase")]
    ObjectId { object_name: String },
    #[serde(rename_all = "camelCase")]
    Enum { enum_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
pub enum MapValueType {
    #[serde(rename_all = "camelCase")]
    String {
        format: StringFormat,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_length: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_length: Option<usize>,
    },
    #[serde(rename_all = "camelCase")]
    Number {
        format: NumberFormat,
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<Number>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<Number>,
    },
    #[serde(rename_all = "camelCase")]
    Enum { enum_name: String },
    #[serde(rename_all = "camelCase")]
    Object { object_name: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StringFormat {
    String,
    IpAddress,
    IpNetwork,
    SocketAddress,
    EmailAddress,
    Secret,
    SecretText,
    Uri,
    Color,
    Text,
    Html,
    Sieve,
    #[serde(other)]
    Unknown,
}

impl Filter {
    pub fn field(&self) -> &str {
        match self {
            Filter::Text { field, .. }
            | Filter::Enum { field, .. }
            | Filter::Integer { field, .. }
            | Filter::Date { field, .. }
            | Filter::ObjectId { field, .. } => field,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Filter::Text { label, .. }
            | Filter::Enum { label, .. }
            | Filter::Integer { label, .. }
            | Filter::Date { label, .. }
            | Filter::ObjectId { label, .. } => label,
        }
    }
}

impl ObjectSchema {
    pub fn variant_of(&self, obj: &Map<String, Value>) -> Option<&ObjectVariant> {
        let ObjectSchema::Multiple { variants } = self else {
            return None;
        };
        let at_type = obj.get(AT_TYPE)?.as_str()?;
        variants.iter().find(|v| v.name == at_type)
    }

    pub fn single_name(&self) -> Option<&str> {
        match self {
            ObjectSchema::Single { schema_name } => Some(schema_name),
            ObjectSchema::Multiple { .. } => None,
        }
    }
}

impl FieldType {
    pub fn references(&self, object: &str) -> bool {
        match self {
            FieldType::Object { object_name, .. }
            | FieldType::ObjectList { object_name, .. }
            | FieldType::ObjectId { object_name, .. } => object_name == object,
            FieldType::Set { class, .. } => class.references(object),
            FieldType::Map {
                key_class,
                value_class,
                ..
            } => {
                key_class.references(object)
                    || matches!(
                        value_class,
                        MapValueType::Object { object_name } if object_name == object
                    )
            }
            _ => false,
        }
    }

    pub fn is_nullable(&self) -> bool {
        match self {
            FieldType::String { nullable, .. }
            | FieldType::Number { nullable, .. }
            | FieldType::UtcDateTime { nullable }
            | FieldType::Enum { nullable, .. }
            | FieldType::ObjectId { nullable, .. }
            | FieldType::Object { nullable, .. } => *nullable,
            _ => false,
        }
    }

    pub fn is_composite(&self) -> bool {
        matches!(
            self,
            FieldType::Object { .. }
                | FieldType::ObjectList { .. }
                | FieldType::Set { .. }
                | FieldType::Map { .. }
        )
    }

    pub fn is_nested_object(&self) -> bool {
        matches!(
            self,
            FieldType::Object { .. }
                | FieldType::ObjectList { .. }
                | FieldType::Map {
                    value_class: MapValueType::Object { .. },
                    ..
                }
        )
    }
}

impl FormField {
    pub fn label_or_name(&self) -> &str {
        if self.label.is_empty() {
            &self.name
        } else {
            &self.label
        }
    }
}

impl ScalarType {
    pub fn references(&self, object: &str) -> bool {
        matches!(self, ScalarType::ObjectId { object_name } if object_name == object)
    }

    pub fn field_type(&self) -> FieldType {
        match self {
            ScalarType::String {
                format,
                min_length,
                max_length,
            } => FieldType::String {
                format: *format,
                min_length: *min_length,
                max_length: *max_length,
                nullable: false,
            },
            ScalarType::ObjectId { object_name } => FieldType::ObjectId {
                object_name: object_name.clone(),
                nullable: false,
            },
            ScalarType::Enum { enum_name } => FieldType::Enum {
                enum_name: enum_name.clone(),
                nullable: false,
            },
        }
    }
}

impl MapValueType {
    pub fn field_type(&self) -> FieldType {
        match self {
            MapValueType::String {
                format,
                min_length,
                max_length,
            } => FieldType::String {
                format: *format,
                min_length: *min_length,
                max_length: *max_length,
                nullable: false,
            },
            MapValueType::Number { format, min, max } => FieldType::Number {
                format: *format,
                min: min.clone(),
                max: max.clone(),
                nullable: false,
            },
            MapValueType::Enum { enum_name } => FieldType::Enum {
                enum_name: enum_name.clone(),
                nullable: false,
            },
            MapValueType::Object { object_name } => FieldType::Object {
                object_name: object_name.clone(),
                nullable: false,
            },
        }
    }
}

impl StringFormat {
    pub fn is_multiline(&self) -> bool {
        matches!(
            self,
            StringFormat::SecretText
                | StringFormat::Text
                | StringFormat::Html
                | StringFormat::Sieve
                | StringFormat::Unknown
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NumberFormat {
    Integer,
    UnsignedInteger,
    Float,
    Size,
    Duration,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FieldUpdate {
    Mutable,
    Immutable,
    ServerSet,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnumVariant {
    pub name: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_format: Option<NumberFormat>,
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/schema/types/tests.rs"]
mod tests;
