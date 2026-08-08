/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::collections::HashMap;

use crate::jmap::errors::{ObjectRef, SetError, SetErrorType, ValidationError};
use crate::render::Ansi;

pub type ClientIds<'a> = HashMap<&'a str, &'a str>;

pub fn render(err: &SetError, ansi: Ansi) -> String {
    render_with_refs(err, ansi, &ClientIds::new())
}

pub fn render_with_refs(err: &SetError, ansi: Ansi, client_ids: &ClientIds<'_>) -> String {
    let mut out = String::new();
    out.push_str(ansi.red());
    out.push_str("error: ");
    out.push_str(set_error_type_name(&err.type_));
    out.push_str(ansi.reset());
    if let Some(desc) = &err.description
        && !desc.is_empty()
    {
        out.push('\n');
        out.push_str("  ");
        out.push_str(desc);
    }
    if let Some(props) = &err.properties
        && !props.is_empty()
    {
        out.push('\n');
        out.push_str("  Properties: ");
        join_into(&mut out, props.iter().map(String::as_str), ", ");
    }
    if let Some(id) = &err.existing_id {
        out.push('\n');
        out.push_str("  Existing id: ");
        out.push_str(id);
        push_client_id(&mut out, id, client_ids);
    }
    if let Some(oref) = &err.object_id {
        out.push('\n');
        out.push_str("  Object id:   ");
        push_object_ref(&mut out, oref, client_ids);
    }
    if !err.linked_objects.is_empty() {
        out.push('\n');
        out.push_str("  Linked by:   ");
        let mut first = true;
        for oref in &err.linked_objects {
            if !first {
                out.push_str(", ");
            }
            first = false;
            push_object_ref(&mut out, oref, client_ids);
        }
    }
    if !err.validation_errors.is_empty() {
        out.push('\n');
        out.push_str("  Validation errors:");
        for v in &err.validation_errors {
            out.push('\n');
            out.push_str("    ");
            render_validation_error(&mut out, v);
        }
    }
    out
}

fn render_validation_error(out: &mut String, err: &ValidationError) {
    match err {
        ValidationError::Invalid { property, value } => {
            out.push_str(property);
            out.push_str(": invalid value \"");
            out.push_str(value);
            out.push('"');
        }
        ValidationError::Required { property } => {
            out.push_str(property);
            out.push_str(": required");
        }
        ValidationError::MaxLength { property, required } => {
            out.push_str(property);
            out.push_str(": must be at most ");
            out.push_str(&required.to_string());
            out.push_str(" chars");
        }
        ValidationError::MinLength { property, required } => {
            out.push_str(property);
            out.push_str(": must be at least ");
            out.push_str(&required.to_string());
            out.push_str(" chars");
        }
        ValidationError::MaxValue { property, required } => {
            out.push_str(property);
            out.push_str(": must be at most ");
            out.push_str(&required.to_string());
        }
        ValidationError::MinValue { property, required } => {
            out.push_str(property);
            out.push_str(": must be at least ");
            out.push_str(&required.to_string());
        }
    }
}

fn join_into<'a, I: Iterator<Item = &'a str>>(out: &mut String, iter: I, sep: &str) {
    let mut first = true;
    for s in iter {
        if !first {
            out.push_str(sep);
        }
        first = false;
        out.push_str(s);
    }
}

fn push_object_ref(out: &mut String, oref: &ObjectRef, client_ids: &ClientIds<'_>) {
    match oref.object() {
        Some(obj) => {
            out.push_str(obj.strip_prefix("x:").unwrap_or(obj));
            out.push('#');
            out.push_str(oref.id());
        }
        None => out.push_str(oref.id()),
    }
    push_client_id(out, oref.id(), client_ids);
}

fn push_client_id(out: &mut String, server_id: &str, client_ids: &ClientIds<'_>) {
    if let Some(client_id) = client_ids.get(server_id) {
        out.push_str(" (plan reference #");
        out.push_str(client_id);
        out.push(')');
    }
}

pub fn set_error_type_name(t: &SetErrorType) -> &'static str {
    match t {
        SetErrorType::Forbidden => "forbidden",
        SetErrorType::OverQuota => "overQuota",
        SetErrorType::TooLarge => "tooLarge",
        SetErrorType::RateLimit => "rateLimit",
        SetErrorType::NotFound => "notFound",
        SetErrorType::InvalidPatch => "invalidPatch",
        SetErrorType::WillDestroy => "willDestroy",
        SetErrorType::InvalidProperties => "invalidProperties",
        SetErrorType::Singleton => "singleton",
        SetErrorType::MailboxHasChild => "mailboxHasChild",
        SetErrorType::MailboxHasEmail => "mailboxHasEmail",
        SetErrorType::BlobNotFound => "blobNotFound",
        SetErrorType::TooManyKeywords => "tooManyKeywords",
        SetErrorType::TooManyMailboxes => "tooManyMailboxes",
        SetErrorType::ForbiddenFrom => "forbiddenFrom",
        SetErrorType::InvalidEmail => "invalidEmail",
        SetErrorType::TooManyRecipients => "tooManyRecipients",
        SetErrorType::NoRecipients => "noRecipients",
        SetErrorType::InvalidRecipients => "invalidRecipients",
        SetErrorType::ForbiddenMailFrom => "forbiddenMailFrom",
        SetErrorType::ForbiddenToSend => "forbiddenToSend",
        SetErrorType::CannotUnsend => "cannotUnsend",
        SetErrorType::AlreadyExists => "alreadyExists",
        SetErrorType::InvalidScript => "invalidScript",
        SetErrorType::ScriptIsActive => "scriptIsActive",
        SetErrorType::AddressBookHasContents => "addressBookHasContents",
        SetErrorType::NodeHasChildren => "nodeHasChildren",
        SetErrorType::CalendarHasEvent => "calendarHasEvent",
        SetErrorType::ObjectIsLinked => "objectIsLinked",
        SetErrorType::InvalidForeignKey => "invalidForeignKey",
        SetErrorType::PrimaryKeyViolation => "primaryKeyViolation",
        SetErrorType::ValidationFailed => "validationFailed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_validation_errors() {
        let err = SetError {
            type_: SetErrorType::ValidationFailed,
            description: Some("bad".into()),
            validation_errors: vec![
                ValidationError::Required {
                    property: "/name".into(),
                },
                ValidationError::MinLength {
                    property: "/name".into(),
                    required: 3,
                },
            ],
            ..SetError::default()
        };
        let s = render(&err, Ansi::new(false));
        assert!(s.contains("validationFailed"));
        assert!(s.contains("/name: required"));
        assert!(s.contains("at least 3 chars"));
    }

    #[test]
    fn annotates_object_id_with_plan_reference() {
        let err = SetError {
            type_: SetErrorType::InvalidForeignKey,
            object_id: Some(ObjectRef::Structured {
                object: Some("x:DnsServer".into()),
                id: "i1nk7i22boqc".into(),
            }),
            ..SetError::default()
        };
        let mut client_ids = ClientIds::new();
        client_ids.insert("i1nk7i22boqc", "dnsserver-ovh");

        let s = render_with_refs(&err, Ansi::new(false), &client_ids);
        assert!(s.contains("DnsServer#i1nk7i22boqc (plan reference #dnsserver-ovh)"));
    }

    #[test]
    fn leaves_object_id_bare_without_a_matching_reference() {
        let err = SetError {
            type_: SetErrorType::InvalidForeignKey,
            object_id: Some(ObjectRef::Structured {
                object: Some("x:DnsServer".into()),
                id: "i1nk7i22boqc".into(),
            }),
            ..SetError::default()
        };
        let s = render(&err, Ansi::new(false));
        assert!(s.contains("DnsServer#i1nk7i22boqc"));
        assert!(!s.contains("plan reference"));
    }

    #[test]
    fn annotates_linked_objects_and_existing_id() {
        let err = SetError {
            type_: SetErrorType::ObjectIsLinked,
            existing_id: Some("abc".into()),
            linked_objects: vec![
                ObjectRef::Structured {
                    object: Some("x:Account".into()),
                    id: "acc1".into(),
                },
                ObjectRef::Id("acc2".into()),
            ],
            ..SetError::default()
        };
        let mut client_ids = ClientIds::new();
        client_ids.insert("abc", "dom-a");
        client_ids.insert("acc1", "user-1");

        let s = render_with_refs(&err, Ansi::new(false), &client_ids);
        assert!(s.contains("Existing id: abc (plan reference #dom-a)"));
        assert!(s.contains("Account#acc1 (plan reference #user-1)"));
        assert!(s.contains("acc2"));
    }
}
