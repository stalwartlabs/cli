/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("unknown object: {0}")]
    UnknownObject(String),
    #[error("{0} is a view; use the underlying object name")]
    ObjectIsView(String),
    #[error("unknown variant '{variant}' for object {object}")]
    UnknownVariant { object: String, variant: String },
    #[error("{0} is not multi-variant; do not use the `/Variant` syntax")]
    NotMultiVariant(String),
    #[error("the Object/Variant syntax is only valid for `create`")]
    VariantOutsideCreate,
    #[error("{0}")]
    Embedded(EmbeddedObject),
}

#[derive(Debug)]
pub struct EmbeddedObject {
    pub name: String,
    pub holders: Vec<String>,
    pub field: Option<String>,
}

impl fmt::Display for EmbeddedObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = &self.name;
        if self.holders.is_empty() {
            return write!(
                f,
                "`{name}` is an embedded sub-object, not a top-level object"
            );
        }
        write!(f, "`{name}` is an embedded property of ")?;
        self.write_holders(f)?;
        f.write_str(", not a separate object. Snapshot ")?;
        self.write_holders(f)?;
        match (self.holders.as_slice(), &self.field) {
            ([_], Some(field)) => write!(f, " to capture its {field}."),
            _ => f.write_str(" to capture it."),
        }
    }
}

impl EmbeddedObject {
    fn write_holders(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.holders.as_slice() {
            [] => Ok(()),
            [single] => f.write_str(single),
            [a, b] => write!(f, "{a} or {b}"),
            [init @ .., last] => {
                for holder in init {
                    write!(f, "{holder}, ")?;
                }
                write!(f, "or {last}")
            }
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/schema/error/tests.rs"]
mod tests;
