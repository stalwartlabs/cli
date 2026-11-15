/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use serde::de::{Error as _, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SetError {
    #[serde(rename = "type")]
    pub type_: SetErrorType,

    pub description: Option<String>,

    pub properties: Option<Vec<String>>,

    #[serde(rename = "existingId")]
    pub existing_id: Option<String>,

    #[serde(rename = "objectId")]
    pub object_id: Option<ObjectRef>,

    #[serde(rename = "linkedObjects")]
    pub linked_objects: Vec<ObjectRef>,

    #[serde(rename = "validationErrors", deserialize_with = "validation_errors")]
    pub validation_errors: Vec<ValidationError>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ObjectRef {
    Structured {
        #[serde(default)]
        object: Option<String>,
        id: String,
    },
    Id(String),
}

impl ObjectRef {
    pub fn id(&self) -> &str {
        match self {
            ObjectRef::Structured { id, .. } => id,
            ObjectRef::Id(s) => s,
        }
    }

    pub fn object(&self) -> Option<&str> {
        match self {
            ObjectRef::Structured { object, .. } => object.as_deref(),
            ObjectRef::Id(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type")]
pub enum ValidationError {
    Invalid {
        property: String,
        value: String,
    },
    Required {
        property: String,
    },
    MaxLength {
        property: String,
        required: usize,
    },
    MinLength {
        property: String,
        required: usize,
    },
    MaxValue {
        property: String,
        required: i64,
    },
    MinValue {
        property: String,
        required: i64,
    },
    #[serde(skip)]
    Other {
        kind: Box<str>,
        property: Option<String>,
    },
}

impl ValidationError {
    pub fn property(&self) -> Option<&str> {
        match self {
            ValidationError::Invalid { property, .. }
            | ValidationError::Required { property }
            | ValidationError::MaxLength { property, .. }
            | ValidationError::MinLength { property, .. }
            | ValidationError::MaxValue { property, .. }
            | ValidationError::MinValue { property, .. } => Some(property),
            ValidationError::Other { property, .. } => property.as_deref(),
        }
    }
}

fn validation_errors<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ValidationError>, D::Error> {
    let raw = Option::<Vec<Value>>::deserialize(deserializer)?.unwrap_or_default();
    raw.iter()
        .map(|entry| {
            if !entry.is_object() {
                return Err(D::Error::custom("validation error is not an object"));
            }
            Ok(
                ValidationError::deserialize(entry).unwrap_or_else(|_| ValidationError::Other {
                    kind: entry
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .into(),
                    property: entry
                        .get("property")
                        .and_then(Value::as_str)
                        .map(Into::into),
                }),
            )
        })
        .collect()
}

macro_rules! set_error_types {
    ($($(#[$attr:meta])* $variant:ident => $name:literal, $label:literal,)*) => {
        #[derive(Debug, Clone, PartialEq, Eq, Default)]
        pub enum SetErrorType {
            $($(#[$attr])* $variant,)*
            Other(Box<str>),
        }

        impl SetErrorType {
            pub fn as_str(&self) -> &str {
                match self {
                    $(SetErrorType::$variant => $name,)*
                    SetErrorType::Other(name) => name,
                }
            }

            pub fn label(&self) -> Option<&'static str> {
                match self {
                    $(SetErrorType::$variant => Some($label),)*
                    SetErrorType::Other(_) => None,
                }
            }

            fn from_name(name: &str) -> SetErrorType {
                hashify::map!(name.as_bytes(), SetErrorType, $($name => SetErrorType::$variant,)*)
                    .cloned()
                    .unwrap_or_else(|| SetErrorType::Other(name.into()))
            }
        }

        #[cfg(all(test, stalwart_cli_tests))]
        const ALL_SET_ERROR_TYPES: &[SetErrorType] = &[$(SetErrorType::$variant,)*];
    };
}

set_error_types! {
    Forbidden => "forbidden", "Forbidden",
    OverQuota => "overQuota", "Over quota",
    TooLarge => "tooLarge", "Too large",
    RateLimit => "rateLimit", "Rate limited",
    NotFound => "notFound", "Not found",
    InvalidPatch => "invalidPatch", "Invalid patch",
    WillDestroy => "willDestroy", "Object will be destroyed",
    #[default] InvalidProperties => "invalidProperties", "Invalid properties",
    Singleton => "singleton", "Singleton",
    MailboxHasChild => "mailboxHasChild", "Mailbox has child mailboxes",
    MailboxHasEmail => "mailboxHasEmail", "Mailbox has email",
    BlobNotFound => "blobNotFound", "Blob not found",
    TooManyKeywords => "tooManyKeywords", "Too many keywords",
    TooManyMailboxes => "tooManyMailboxes", "Too many mailboxes",
    ForbiddenFrom => "forbiddenFrom", "Forbidden From address",
    InvalidEmail => "invalidEmail", "Invalid email",
    TooManyRecipients => "tooManyRecipients", "Too many recipients",
    NoRecipients => "noRecipients", "No recipients",
    InvalidRecipients => "invalidRecipients", "Invalid recipients",
    ForbiddenMailFrom => "forbiddenMailFrom", "Forbidden MAIL FROM",
    ForbiddenToSend => "forbiddenToSend", "Forbidden to send",
    CannotUnsend => "cannotUnsend", "Cannot unsend",
    AlreadyExists => "alreadyExists", "Already exists",
    InvalidScript => "invalidScript", "Invalid script",
    ScriptIsActive => "scriptIsActive", "Script is active",
    AddressBookHasContents => "addressBookHasContents", "Address book has contents",
    NodeHasChildren => "nodeHasChildren", "Node has children",
    CalendarHasEvent => "calendarHasEvent", "Calendar has events",
    ObjectIsLinked => "objectIsLinked", "Object is linked",
    InvalidForeignKey => "invalidForeignKey", "Invalid reference",
    PrimaryKeyViolation => "primaryKeyViolation", "Primary key violation",
    ValidationFailed => "validationFailed", "Validation failed",
    NoSupportedScheduleMethods => "noSupportedScheduleMethods", "No supported scheduling methods",
}

impl<'de> Deserialize<'de> for SetErrorType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NameVisitor;

        impl Visitor<'_> for NameVisitor {
            type Value = SetErrorType;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a SetError type name")
            }

            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<SetErrorType, E> {
                Ok(SetErrorType::from_name(name))
            }
        }

        deserializer.deserialize_str(NameVisitor)
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/set_error/tests.rs"]
mod tests;
