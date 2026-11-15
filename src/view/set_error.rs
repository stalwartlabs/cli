/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::{self, Write};

use crate::jmap::set_error::{ObjectRef, SetError, ValidationError};
use crate::view::styled::{Style, Styled};

#[derive(Debug, Default)]
pub struct RefLabels(HashMap<Box<str>, Box<str>>);

impl RefLabels {
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self::from_pair_iter(&mut pairs.into_iter())
    }

    fn from_pair_iter<'a>(pairs: &mut dyn Iterator<Item = (&'a str, &'a str)>) -> Self {
        let mut labels: HashMap<Box<str>, Box<str>> = HashMap::new();
        let mut conflicting: Vec<&str> = Vec::new();
        for (server_id, client_id) in pairs {
            match labels.entry(server_id.into()) {
                Entry::Vacant(slot) => {
                    slot.insert(client_id.into());
                }
                Entry::Occupied(slot) => {
                    if **slot.get() != *client_id {
                        conflicting.push(server_id);
                    }
                }
            }
        }
        for server_id in conflicting {
            labels.remove(server_id);
        }
        RefLabels(labels)
    }

    pub fn get(&self, server_id: &str) -> Option<&str> {
        self.0.get(server_id).map(AsRef::as_ref)
    }
}

const DETAIL_LABEL_WIDTH: usize = 11;

pub fn set_error(err: &SetError, labels: &RefLabels, out: &mut Styled) {
    out.push("error:", Style::ErrorTag);
    out.plain(" ");
    set_error_body(err, labels, out);
}

pub fn set_error_body(err: &SetError, labels: &RefLabels, out: &mut Styled) {
    match err.type_.label() {
        Some(label) => {
            out.plain(label);
            out.plain(" ");
            out.push("(", Style::Dim);
            out.push(err.type_.as_str(), Style::Dim);
            out.push(")", Style::Dim);
        }
        None => out.plain(err.type_.as_str()),
    }
    if let Some(desc) = &err.description
        && !desc.is_empty()
    {
        out.plain("\n  ");
        out.plain(desc);
    }
    if let Some(props) = &err.properties
        && let Some((first, rest)) = props.split_first()
    {
        detail_label(
            out,
            if rest.is_empty() {
                "Property:"
            } else {
                "Properties:"
            },
        );
        out.plain(first);
        for prop in rest {
            out.plain(", ");
            out.plain(prop);
        }
    }
    if let Some(id) = &err.existing_id {
        detail_label(out, "Existing:");
        out.plain(id);
        push_plan_reference(out, id, labels);
    }
    if let Some(oref) = &err.object_id {
        detail_label(out, "Object:");
        push_object_ref(out, oref, labels);
    }
    if let Some((first, rest)) = err.linked_objects.split_first() {
        detail_label(out, "Linked by:");
        push_object_ref(out, first, labels);
        for oref in rest {
            out.plain(", ");
            push_object_ref(out, oref, labels);
        }
    }
    if !err.validation_errors.is_empty() {
        out.plain("\n  Validation:");
        for v in &err.validation_errors {
            out.plain("\n    ");
            push_validation_error(out, v);
        }
    }
}

fn detail_label(out: &mut Styled, label: &str) {
    let _ = write!(out, "\n  {label:<width$}", width = DETAIL_LABEL_WIDTH + 1);
}

pub fn set_error_plain(err: &SetError, labels: &RefLabels) -> String {
    let mut out = Styled::default();
    set_error(err, labels, &mut out);
    out.into_text()
}

fn push_validation_error(out: &mut Styled, err: &ValidationError) {
    if let Some(property) = err.property() {
        let _ = write!(out, "{property}: ");
    }
    let _ = err.message(out);
}

impl ValidationError {
    pub fn message(&self, out: &mut impl fmt::Write) -> fmt::Result {
        match self {
            ValidationError::Invalid { value, .. } => write!(out, "invalid value \"{value}\""),
            ValidationError::Required { .. } => out.write_str("required"),
            ValidationError::MaxLength { required, .. } => {
                write!(out, "must be at most {required} chars")
            }
            ValidationError::MinLength { required, .. } => {
                write!(out, "must be at least {required} chars")
            }
            ValidationError::MaxValue { required, .. } => write!(out, "must be at most {required}"),
            ValidationError::MinValue { required, .. } => {
                write!(out, "must be at least {required}")
            }
            ValidationError::Other { kind, .. } => out.write_str(kind),
        }
    }
}

fn push_object_ref(out: &mut Styled, oref: &ObjectRef, labels: &RefLabels) {
    if let Some(obj) = oref.object() {
        out.plain(obj.strip_prefix("x:").unwrap_or(obj));
        out.plain(" ");
    }
    out.plain(oref.id());
    push_plan_reference(out, oref.id(), labels);
}

fn push_plan_reference(out: &mut Styled, server_id: &str, labels: &RefLabels) {
    if let Some(client_id) = labels.get(server_id) {
        out.plain(" (plan reference #");
        out.plain(client_id);
        out.plain(")");
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/view/set_error/tests.rs"]
pub(crate) mod tests;
