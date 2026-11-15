/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::jmap::http::HttpClient;
use crate::jmap::method::MethodName;
use crate::jmap::pages::{Fetch, Pages};
use crate::jmap::response::{GetResponse, SetResponse};
use crate::jmap::session::Session;
use serde::de::DeserializeOwned;
use serde::ser::{SerializeMap, SerializeTuple};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use serde_json::value::RawValue;
use std::collections::HashMap;

const USING: &[&str] = &["urn:ietf:params:jmap:core", "urn:stalwart:jmap"];
const STALWART_PREFIX: &str = "x:";
const SINGLE_CALL_ID: &str = "c0";
const EMPTY_OBJECT: &str = "{}";
const UNKNOWN_ERROR_TYPE: &str = "(unknown)";

type MethodCall = (String, Value, String);

pub struct Client {
    http: HttpClient,
    session: Session,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CallScope<'a> {
    #[default]
    Stalwart,
    Account(&'a str),
}

#[derive(Default)]
pub struct Request<'a> {
    scope: CallScope<'a>,
    calls: Vec<Call<'a>>,
    created_ids: Option<Box<RawValue>>,
}

struct Envelope<'r, 'a> {
    using: Using<'r>,
    request: &'r Request<'a>,
}

#[derive(Clone, Copy)]
enum Using<'a> {
    Stalwart,
    Account {
        session: &'a [Box<str>],
        account: &'a [Box<str>],
    },
}

#[derive(Serialize)]
struct WithAccount<'a, T: Serialize> {
    #[serde(rename = "accountId")]
    account_id: &'a str,
    #[serde(flatten)]
    args: &'a T,
}

struct Call<'a> {
    method: MethodName<'a>,
    args: Box<RawValue>,
    id: &'static str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    #[serde(rename = "methodResponses")]
    responses: Vec<MethodCall>,
    created_ids: Option<HashMap<String, String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultRef<'a> {
    pub result_of: &'a str,
    pub name: MethodName<'a>,
    pub path: &'a str,
}

#[derive(Serialize)]
struct GetArgs<'a> {
    ids: &'a [&'a str],
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<&'a [&'a str]>,
}

impl Client {
    pub fn connect(http: HttpClient) -> Result<Self> {
        let session = Session::fetch(&http)?;
        Ok(Client { http, session })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn max_get(&self) -> usize {
        self.session.max_objects_in_get.max(1)
    }

    pub fn max_set(&self) -> usize {
        self.session.max_objects_in_set.max(1)
    }

    pub fn max_calls(&self) -> usize {
        self.session.max_calls_in_request.max(1)
    }

    pub fn send(&self, request: &Request<'_>) -> Result<Response> {
        let using = match request.scope {
            CallScope::Account(id)
                if request
                    .calls
                    .iter()
                    .any(|call| !call.method.object.starts_with(STALWART_PREFIX)) =>
            {
                Using::Account {
                    session: &self.session.capabilities,
                    account: self
                        .session
                        .account(id)
                        .map_or(&[], |account| &account.capabilities),
                }
            }
            CallScope::Account(_) | CallScope::Stalwart => Using::Stalwart,
        };
        let body = serde_json::to_vec(&Envelope { using, request })?;
        self.http
            .post_json(&self.session.api_path, body)?
            .json()
            .map_err(Error::from)
    }

    pub fn get(
        &self,
        object: &str,
        ids: &[&str],
        properties: Option<&[&str]>,
    ) -> Result<GetResponse> {
        self.get_in(CallScope::Stalwart, object, ids, properties)
    }

    pub fn get_in(
        &self,
        scope: CallScope<'_>,
        object: &str,
        ids: &[&str],
        properties: Option<&[&str]>,
    ) -> Result<GetResponse> {
        self.call_as(scope, MethodName::get(object), &GetArgs { ids, properties })
    }

    pub fn set(&self, object: &str, args: &impl Serialize) -> Result<SetResponse> {
        self.set_in(CallScope::Stalwart, object, args)
    }

    pub fn set_in(
        &self,
        scope: CallScope<'_>,
        object: &str,
        args: &impl Serialize,
    ) -> Result<SetResponse> {
        self.call_as(scope, MethodName::set(object), args)
    }

    pub fn pages<'a>(
        &'a self,
        object: &'a str,
        filter: Option<&'a Value>,
        fetch: Fetch<'a>,
    ) -> Pages<'a> {
        self.pages_in(CallScope::Stalwart, object, filter, fetch)
    }

    pub fn pages_in<'a>(
        &'a self,
        scope: CallScope<'a>,
        object: &'a str,
        filter: Option<&'a Value>,
        fetch: Fetch<'a>,
    ) -> Pages<'a> {
        Pages::new(self, scope, object, filter, fetch, self.max_get())
    }

    pub(super) fn call_as<T: DeserializeOwned>(
        &self,
        scope: CallScope<'_>,
        method: MethodName<'_>,
        args: &impl Serialize,
    ) -> Result<T> {
        let mut request = Request::scoped(scope);
        request.call(method, args, SINGLE_CALL_ID)?;
        self.send(&request)?.take_as(SINGLE_CALL_ID, method)
    }
}

impl<'a> CallScope<'a> {
    fn account_for(self, object: &str) -> Option<&'a str> {
        match self {
            CallScope::Account(id) if !object.starts_with(STALWART_PREFIX) => Some(id),
            _ => None,
        }
    }
}

impl<'a> Request<'a> {
    pub fn new() -> Self {
        Request::default()
    }

    pub fn scoped(scope: CallScope<'a>) -> Self {
        Request {
            scope,
            ..Request::default()
        }
    }

    pub fn call(
        &mut self,
        method: MethodName<'a>,
        args: &impl Serialize,
        id: &'static str,
    ) -> Result<&mut Self> {
        let args = match self.scope.account_for(method.object) {
            Some(account_id) => serde_json::value::to_raw_value(&WithAccount { account_id, args })?,
            None => serde_json::value::to_raw_value(args)?,
        };
        self.calls.push(Call { method, args, id });
        Ok(self)
    }

    pub fn created_ids(&mut self, ids: &impl Serialize) -> Result<&mut Self> {
        let ids = serde_json::value::to_raw_value(ids)?;
        self.created_ids = (ids.get() != EMPTY_OBJECT).then_some(ids);
        Ok(self)
    }
}

impl Serialize for Envelope<'_, '_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let created_ids = self.request.created_ids.as_deref();
        let mut map = serializer.serialize_map(Some(2 + usize::from(created_ids.is_some())))?;
        map.serialize_entry("using", &self.using)?;
        map.serialize_entry("methodCalls", &self.request.calls)?;
        if let Some(ids) = created_ids {
            map.serialize_entry("createdIds", ids)?;
        }
        map.end()
    }
}

impl Serialize for Using<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match *self {
            Using::Stalwart => serializer.collect_seq(USING),
            Using::Account { session, account } => serializer.collect_seq(
                session
                    .iter()
                    .chain(account.iter().filter(|name| !session.contains(name))),
            ),
        }
    }
}

impl Serialize for Call<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut tuple = serializer.serialize_tuple(3)?;
        tuple.serialize_element(&self.method)?;
        tuple.serialize_element(&self.args)?;
        tuple.serialize_element(self.id)?;
        tuple.end()
    }
}

impl Response {
    pub fn take(&mut self, id: &str, expected: MethodName<'_>) -> Result<Value> {
        let position = self
            .responses
            .iter()
            .position(|(_, _, call_id)| call_id == id)
            .ok_or_else(|| Error::UnexpectedResponse(format!("missing response `{id}`")))?;
        let (name, value, _) = self.responses.swap_remove(position);
        if name == "error" {
            return Err(Self::method_error(&value));
        }
        if !expected.matches(&name) {
            return Err(Error::UnexpectedResponse(format!(
                "expected response `{expected}`, got `{name}`"
            )));
        }
        Ok(value)
    }

    pub fn take_as<T: DeserializeOwned>(
        &mut self,
        id: &str,
        expected: MethodName<'_>,
    ) -> Result<T> {
        serde_json::from_value(self.take(id, expected)?)
            .map_err(|e| Error::UnexpectedResponse(format!("invalid `{expected}` response: {e}")))
    }

    pub fn created_ids(&self) -> Option<&HashMap<String, String>> {
        self.created_ids.as_ref()
    }

    fn method_error(value: &Value) -> Error {
        let text = |key| value.get(key).and_then(Value::as_str);
        Error::JmapMethod {
            type_: text("type").unwrap_or(UNKNOWN_ERROR_TYPE).to_string(),
            description: text("description").unwrap_or_default().to_string(),
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/client/tests.rs"]
mod tests;

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/client/test_support.rs"]
mod test_support;
