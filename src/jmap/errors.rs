/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

    #[serde(rename = "validationErrors")]
    pub validation_errors: Vec<ValidationError>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum ValidationError {
    Invalid { property: String, value: String },
    Required { property: String },
    MaxLength { property: String, required: usize },
    MinLength { property: String, required: usize },
    MaxValue { property: String, required: i64 },
    MinValue { property: String, required: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SetErrorType {
    #[serde(rename = "forbidden")]
    Forbidden,
    #[serde(rename = "overQuota")]
    OverQuota,
    #[serde(rename = "tooLarge")]
    TooLarge,
    #[serde(rename = "rateLimit")]
    RateLimit,
    #[serde(rename = "notFound")]
    NotFound,
    #[serde(rename = "invalidPatch")]
    InvalidPatch,
    #[serde(rename = "willDestroy")]
    WillDestroy,
    #[default]
    #[serde(rename = "invalidProperties")]
    InvalidProperties,
    #[serde(rename = "singleton")]
    Singleton,
    #[serde(rename = "mailboxHasChild")]
    MailboxHasChild,
    #[serde(rename = "mailboxHasEmail")]
    MailboxHasEmail,
    #[serde(rename = "blobNotFound")]
    BlobNotFound,
    #[serde(rename = "tooManyKeywords")]
    TooManyKeywords,
    #[serde(rename = "tooManyMailboxes")]
    TooManyMailboxes,
    #[serde(rename = "forbiddenFrom")]
    ForbiddenFrom,
    #[serde(rename = "invalidEmail")]
    InvalidEmail,
    #[serde(rename = "tooManyRecipients")]
    TooManyRecipients,
    #[serde(rename = "noRecipients")]
    NoRecipients,
    #[serde(rename = "invalidRecipients")]
    InvalidRecipients,
    #[serde(rename = "forbiddenMailFrom")]
    ForbiddenMailFrom,
    #[serde(rename = "forbiddenToSend")]
    ForbiddenToSend,
    #[serde(rename = "cannotUnsend")]
    CannotUnsend,
    #[serde(rename = "alreadyExists")]
    AlreadyExists,
    #[serde(rename = "invalidScript")]
    InvalidScript,
    #[serde(rename = "scriptIsActive")]
    ScriptIsActive,
    #[serde(rename = "addressBookHasContents")]
    AddressBookHasContents,
    #[serde(rename = "nodeHasChildren")]
    NodeHasChildren,
    #[serde(rename = "calendarHasEvent")]
    CalendarHasEvent,

    #[serde(rename = "objectIsLinked")]
    ObjectIsLinked,
    #[serde(rename = "invalidForeignKey")]
    InvalidForeignKey,
    #[serde(rename = "primaryKeyViolation")]
    PrimaryKeyViolation,
    #[serde(rename = "validationFailed")]
    ValidationFailed,
}
