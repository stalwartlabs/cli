/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use serde::{Serialize, Serializer};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Set,
    Query,
    Upload,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MethodName<'a> {
    pub object: &'a str,
    pub method: Method,
}

impl Method {
    pub const fn as_str(self) -> &'static str {
        match self {
            Method::Get => "get",
            Method::Set => "set",
            Method::Query => "query",
            Method::Upload => "upload",
        }
    }
}

impl<'a> MethodName<'a> {
    pub const fn new(object: &'a str, method: Method) -> Self {
        MethodName { object, method }
    }

    pub const fn get(object: &'a str) -> Self {
        MethodName::new(object, Method::Get)
    }

    pub const fn set(object: &'a str) -> Self {
        MethodName::new(object, Method::Set)
    }

    pub const fn query(object: &'a str) -> Self {
        MethodName::new(object, Method::Query)
    }

    pub fn matches(self, name: &str) -> bool {
        name.strip_prefix(self.object)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|method| method == self.method.as_str())
    }
}

impl fmt::Display for MethodName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.object)?;
        f.write_str("/")?;
        f.write_str(self.method.as_str())
    }
}

impl Serialize for MethodName<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/method/tests.rs"]
mod tests;
