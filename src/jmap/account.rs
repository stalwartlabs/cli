/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::jmap::http::HttpClient;
use serde::{Deserialize, Deserializer};
use std::cmp::Ordering;

const ACCOUNT_PATH: &str = "/api/account";
const BOOTSTRAP_GET: &str = "sysBootstrapGet";
const BOOTSTRAP_UPDATE: &str = "sysBootstrapUpdate";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccountInfo {
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub edition: Edition,
    #[serde(default)]
    pub locale: Box<str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Edition {
    #[default]
    Oss,
    Community,
    Enterprise,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Permissions(Box<[Box<str>]>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Get,
    Create,
    Update,
    Destroy,
}

impl AccountInfo {
    pub fn fetch(http: &HttpClient) -> Result<Self> {
        Ok(http.get(ACCOUNT_PATH)?.json()?)
    }
}

impl Edition {
    pub fn parse(raw: &str) -> Self {
        hashify::map!(raw.as_bytes(), Edition,
            "oss" => Edition::Oss,
            "community" => Edition::Community,
            "enterprise" => Edition::Enterprise,
        )
        .copied()
        .unwrap_or_default()
    }
}

impl Op {
    pub const fn suffix(self) -> &'static str {
        match self {
            Op::Get => "Get",
            Op::Create => "Create",
            Op::Update => "Update",
            Op::Destroy => "Destroy",
        }
    }
}

impl Permissions {
    pub fn new(names: impl IntoIterator<Item = Box<str>>) -> Self {
        let mut names: Vec<Box<str>> = names.into_iter().collect();
        names.sort_unstable();
        names.dedup();
        Permissions(names.into_boxed_slice())
    }

    pub fn has(&self, name: &str) -> bool {
        self.find(name, "")
    }

    pub fn can(&self, prefix: &str, op: Op) -> bool {
        self.find(prefix, op.suffix())
    }

    pub fn is_bootstrap(&self) -> bool {
        matches!(&*self.0, [a, b] if &**a == BOOTSTRAP_GET && &**b == BOOTSTRAP_UPDATE)
    }

    fn find(&self, head: &str, tail: &str) -> bool {
        self.0
            .binary_search_by(|name| compare_concat(name, head, tail))
            .is_ok()
    }
}

fn compare_concat(name: &str, head: &str, tail: &str) -> Ordering {
    name.bytes().cmp(head.bytes().chain(tail.bytes()))
}

impl<'de> Deserialize<'de> for Permissions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Permissions::new(Vec::<Box<str>>::deserialize(
            deserializer,
        )?))
    }
}

impl<'de> Deserialize<'de> for Edition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Edition::parse(&String::deserialize(deserializer)?))
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/account/tests.rs"]
mod tests;
