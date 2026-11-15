/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::fmt::{self, Display, Formatter};

use crate::schema::object::display_name;
use crate::schema::{FieldType, Filter, MapValueType, NumberFormat, ScalarType, StringFormat};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Title,
    Lower,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Count {
    One,
    Many,
}

#[derive(Clone, Copy)]
struct Words {
    title: &'static str,
    one: &'static str,
    many: &'static str,
}

impl Words {
    const fn new(title: &'static str, one: &'static str, many: &'static str) -> Self {
        Words { title, one, many }
    }

    fn pick(self, case: Case, count: Count) -> &'static str {
        match (case, count) {
            (Case::Title, Count::One) => self.title,
            (Case::Lower, Count::One) => self.one,
            (_, Count::Many) => self.many,
        }
    }
}

impl StringFormat {
    const fn words(self) -> Words {
        match self {
            StringFormat::String | StringFormat::Unknown => Words::new("Text", "text", "text"),
            StringFormat::IpAddress => Words::new("IP address", "IP address", "IP addresses"),
            StringFormat::IpNetwork => Words::new("IP network", "IP network", "IP networks"),
            StringFormat::SocketAddress => {
                Words::new("Socket address", "socket address", "socket addresses")
            }
            StringFormat::EmailAddress => {
                Words::new("Email address", "email address", "email addresses")
            }
            StringFormat::Secret => Words::new("Secret", "secret", "secrets"),
            StringFormat::SecretText => Words::new("Secret text", "secret text", "secret texts"),
            StringFormat::Uri => Words::new("URI", "URI", "URIs"),
            StringFormat::Color => Words::new("Color", "color", "colors"),
            StringFormat::Text => {
                Words::new("Multi-line text", "multi-line text", "multi-line texts")
            }
            StringFormat::Html => Words::new("HTML", "HTML", "HTML documents"),
            StringFormat::Sieve => Words::new("Sieve script", "Sieve script", "Sieve scripts"),
        }
    }
}

impl NumberFormat {
    const fn words(self) -> Words {
        match self {
            NumberFormat::Integer | NumberFormat::Unknown => {
                Words::new("Integer", "integer", "integers")
            }
            NumberFormat::UnsignedInteger => {
                Words::new("Unsigned integer", "unsigned integer", "unsigned integers")
            }
            NumberFormat::Float => {
                Words::new("Decimal number", "decimal number", "decimal numbers")
            }
            NumberFormat::Size => Words::new("Size in bytes", "size in bytes", "sizes in bytes"),
            NumberFormat::Duration => Words::new(
                "Duration in milliseconds",
                "duration in milliseconds",
                "durations in milliseconds",
            ),
        }
    }
}

pub struct TypeName<'a>(pub &'a FieldType);

pub struct FilterKind<'a>(pub &'a Filter);

struct Reference<'a> {
    object: &'a str,
    case: Case,
    count: Count,
}

struct EnumOf<'a> {
    name: &'a str,
    case: Case,
    count: Count,
}

impl Display for Reference<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let lead = match (self.case, self.count) {
            (Case::Title, _) => "Reference to ",
            (Case::Lower, Count::One) => "reference to ",
            (Case::Lower, Count::Many) => "references to ",
        };
        write!(f, "{lead}{}", display_name(self.object))
    }
}

impl Display for EnumOf<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let name = display_name(self.name);
        match (self.case, self.count) {
            (Case::Title, _) => write!(f, "One of {name}"),
            (Case::Lower, Count::One) => write!(f, "{name} value"),
            (Case::Lower, Count::Many) => write!(f, "{name} values"),
        }
    }
}

impl ScalarType {
    fn write_name(&self, f: &mut Formatter<'_>, count: Count) -> fmt::Result {
        match self {
            ScalarType::String { format, .. } => {
                f.write_str(format.words().pick(Case::Lower, count))
            }
            ScalarType::ObjectId { object_name } => Reference {
                object: object_name,
                case: Case::Lower,
                count,
            }
            .fmt(f),
            ScalarType::Enum { enum_name } => EnumOf {
                name: enum_name,
                case: Case::Lower,
                count,
            }
            .fmt(f),
        }
    }
}

impl MapValueType {
    fn write_name(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            MapValueType::String { format, .. } => {
                f.write_str(format.words().pick(Case::Lower, Count::One))
            }
            MapValueType::Number { format, .. } => {
                f.write_str(format.words().pick(Case::Lower, Count::One))
            }
            MapValueType::Enum { enum_name } => EnumOf {
                name: enum_name,
                case: Case::Lower,
                count: Count::One,
            }
            .fmt(f),
            MapValueType::Object { object_name } => {
                write!(f, "{} object", display_name(object_name))
            }
        }
    }
}

impl Display for TypeName<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            FieldType::String { format, .. } => f.write_str(format.words().title),
            FieldType::Number { format, .. } => f.write_str(format.words().title),
            FieldType::UtcDateTime { .. } => f.write_str("Date and time"),
            FieldType::Boolean => f.write_str("Boolean"),
            FieldType::BlobId => f.write_str("Blob"),
            FieldType::Enum { enum_name, .. } => EnumOf {
                name: enum_name,
                case: Case::Title,
                count: Count::One,
            }
            .fmt(f),
            FieldType::ObjectId { object_name, .. } => Reference {
                object: object_name,
                case: Case::Title,
                count: Count::One,
            }
            .fmt(f),
            FieldType::Object { object_name, .. } => {
                write!(f, "{} object", display_name(object_name))
            }
            FieldType::ObjectList { object_name, .. } => {
                write!(f, "List of {} objects", display_name(object_name))
            }
            FieldType::Set { class, .. } => {
                f.write_str("Set of ")?;
                class.write_name(f, Count::Many)
            }
            FieldType::Map {
                key_class,
                value_class,
                ..
            } => {
                f.write_str("Map from ")?;
                key_class.write_name(f, Count::One)?;
                f.write_str(" to ")?;
                value_class.write_name(f)
            }
        }
    }
}

impl Display for FilterKind<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            Filter::Text { .. } => f.write_str("Text"),
            Filter::Integer { .. } => f.write_str("Integer"),
            Filter::Date { .. } => f.write_str("Date"),
            Filter::Enum { enum_name, .. } => EnumOf {
                name: enum_name,
                case: Case::Title,
                count: Count::One,
            }
            .fmt(f),
            Filter::ObjectId { object_name, .. } => Reference {
                object: object_name,
                case: Case::Title,
                count: Count::One,
            }
            .fmt(f),
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/view/type_name/tests.rs"]
mod tests;
