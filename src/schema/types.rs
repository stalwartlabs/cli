/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Schema {
    pub objects: HashMap<String, ObjectType>,
    pub schemas: HashMap<String, ObjectSchema>,
    pub fields: HashMap<String, Fields>,
    pub forms: HashMap<String, Form>,
    pub lists: HashMap<String, List>,
    pub enums: HashMap<String, Vec<EnumVariant>>,
    pub dashboards: Vec<Dashboard>,
    pub layouts: Vec<Layout>,
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
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub properties: HashMap<String, Field>,

    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub defaults: HashMap<String, Value>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NumberFormat {
    Integer,
    UnsignedInteger,
    Float,
    Size,
    Duration,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub name: String,
    pub icon: String,
    pub items: Vec<LayoutItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LayoutItem {
    #[serde(rename_all = "camelCase")]
    Container {
        name: String,
        icon: String,
        items: Vec<LayoutSubItem>,
    },
    #[serde(rename_all = "camelCase")]
    Link {
        name: String,
        icon: String,
        view_name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
pub enum LayoutSubItem {
    #[serde(rename_all = "camelCase")]
    Container {
        name: String,
        items: Vec<LayoutSubItem>,
    },
    #[serde(rename_all = "camelCase")]
    Link { name: String, view_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<Card>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub charts: Vec<Chart>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub title: String,
    pub icon: String,
    pub source: CardSource,
    pub metrics: Vec<String>,
    #[serde(default)]
    pub aggregate: Aggregate,
    pub format: MetricFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sparkline: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub delta: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub label: String,
    pub metrics: Vec<String>,
    #[serde(default)]
    pub aggregate: Aggregate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Aggregate {
    #[default]
    Sum,
    Avg,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Chart {
    pub title: String,
    pub kind: ChartKind,
    pub series: Vec<Series>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stacked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_format: Option<MetricFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChartKind {
    #[default]
    Line,
    Area,
    Bar,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CardSource {
    #[default]
    Live,
    History,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MetricFormat {
    #[default]
    Number,
    Bytes,
    Duration,
    Percent,
}
