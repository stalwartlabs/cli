/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::jmap::http::HttpClient;
use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

const SESSION_PATH: &str = "/jmap/session";
const DEFAULT_MAX_OBJECTS: usize = 500;
const DEFAULT_MAX_CALLS: usize = 16;
pub const CORE_CAPABILITY: &str = "urn:ietf:params:jmap:core";
const STALWART_CAPABILITY: &str = "urn:stalwart:jmap";

#[derive(Debug, Default)]
pub struct Session {
    pub api_path: String,
    pub max_objects_in_get: usize,
    pub max_objects_in_set: usize,
    pub max_calls_in_request: usize,
    pub capabilities: Box<[Box<str>]>,
    pub accounts: Vec<SessionAccount>,
    pub primary_account: Option<Box<str>>,
    pub username: Box<str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAccount {
    pub id: Box<str>,
    pub name: Box<str>,
    pub is_personal: bool,
    pub capabilities: Box<[Box<str>]>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSession {
    #[serde(default)]
    capabilities: Map<String, Value>,
    api_url: Option<String>,
    #[serde(default)]
    accounts: IndexMap<String, RawAccount>,
    #[serde(default)]
    primary_accounts: HashMap<String, String>,
    #[serde(default)]
    username: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAccount {
    #[serde(default)]
    name: String,
    #[serde(default)]
    is_personal: bool,
    #[serde(default)]
    account_capabilities: Map<String, Value>,
}

#[derive(Deserialize)]
struct RawCore {
    #[serde(rename = "maxObjectsInGet")]
    max_objects_in_get: Option<usize>,
    #[serde(rename = "maxObjectsInSet")]
    max_objects_in_set: Option<usize>,
    #[serde(rename = "maxCallsInRequest")]
    max_calls_in_request: Option<usize>,
}

impl Session {
    pub fn fetch(client: &HttpClient) -> Result<Self> {
        Self::parse(client.get(SESSION_PATH)?.json()?)
    }

    fn parse(mut raw: RawSession) -> Result<Self> {
        let core: RawCore = raw
            .capabilities
            .get_mut(CORE_CAPABILITY)
            .map(Value::take)
            .map(serde_json::from_value)
            .transpose()?
            .ok_or_else(|| {
                Error::UnexpectedResponse("session missing urn:ietf:params:jmap:core".into())
            })?;
        let api_url = raw
            .api_url
            .ok_or_else(|| Error::UnexpectedResponse("session missing apiUrl".into()))?;
        let api_path = absolute_to_path(&api_url);
        let primary_account = raw
            .primary_accounts
            .remove(STALWART_CAPABILITY)
            .map(String::into_boxed_str);

        Ok(Session {
            api_path,
            max_objects_in_get: core.max_objects_in_get.unwrap_or(DEFAULT_MAX_OBJECTS),
            max_objects_in_set: core.max_objects_in_set.unwrap_or(DEFAULT_MAX_OBJECTS),
            max_calls_in_request: core.max_calls_in_request.unwrap_or(DEFAULT_MAX_CALLS),
            capabilities: raw
                .capabilities
                .into_iter()
                .map(|(name, _)| name.into_boxed_str())
                .collect(),
            accounts: raw
                .accounts
                .into_iter()
                .map(|(id, account)| SessionAccount {
                    id: id.into_boxed_str(),
                    name: account.name.into_boxed_str(),
                    is_personal: account.is_personal,
                    capabilities: account
                        .account_capabilities
                        .into_iter()
                        .map(|(name, _)| name.into_boxed_str())
                        .collect(),
                })
                .collect(),
            primary_account,
            username: raw.username.into_boxed_str(),
        })
    }
}

impl Session {
    pub fn account(&self, id: &str) -> Option<&SessionAccount> {
        self.accounts.iter().find(|account| &*account.id == id)
    }
}

impl SessionAccount {
    pub fn supports(&self, object: &str) -> bool {
        standard_capability(object)
            .is_none_or(|capability| self.capabilities.iter().any(|c| &**c == capability))
    }
}

fn standard_capability(object: &str) -> Option<&'static str> {
    hashify::map!(
        object.as_bytes(),
        &str,
        "Mailbox" => "urn:ietf:params:jmap:mail",
        "VacationResponse" => "urn:ietf:params:jmap:vacationresponse",
        "SieveScript" => "urn:ietf:params:jmap:sieve",
        "Calendar" => "urn:ietf:params:jmap:calendars",
        "AddressBook" => "urn:ietf:params:jmap:contacts",
        "Principal" => "urn:ietf:params:jmap:principals",
    )
    .copied()
}

fn absolute_to_path(url: &str) -> String {
    let after = match url.split_once("://") {
        Some((scheme, rest)) if hashify::set_ignore_case!(scheme.as_bytes(), "http", "https") => {
            rest
        }
        _ => return url.trim_end_matches('/').to_string(),
    };
    let path = after.trim_start_matches(|c| c != '/');
    if path.is_empty() {
        "/".to_string()
    } else {
        path.trim_end_matches('/').to_string()
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/session/tests.rs"]
mod tests;
