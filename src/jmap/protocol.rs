/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::error::{CliError, CliResult};
use crate::jmap::http::{HttpClient, status_error};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub const USING: &[&str] = &["urn:ietf:params:jmap:core", "urn:stalwart:jmap"];

#[derive(Serialize)]
struct RequestBody<'a> {
    using: &'a [&'a str],
    #[serde(rename = "methodCalls")]
    method_calls: Vec<MethodCall>,
    #[serde(rename = "createdIds", skip_serializing_if = "Option::is_none")]
    created_ids: Option<&'a HashMap<String, String>>,
}

#[derive(Deserialize)]
struct ResponseBody {
    #[serde(rename = "methodResponses")]
    method_responses: Vec<MethodCall>,
    #[serde(default, rename = "sessionState")]
    _session_state: Option<String>,
    #[serde(default, rename = "createdIds")]
    created_ids: Option<HashMap<String, String>>,
}

pub struct CallResponse {
    pub responses: Vec<MethodCall>,
    pub created_ids: Option<HashMap<String, String>>,
}

pub type MethodCall = (String, Value, String);

pub struct Jmap<'a> {
    client: &'a HttpClient,
    api_path: &'a str,
}

impl<'a> Jmap<'a> {
    pub fn new(client: &'a HttpClient, api_path: &'a str) -> Self {
        Jmap { client, api_path }
    }

    pub fn call_many(&self, calls: Vec<MethodCall>) -> CliResult<Vec<MethodCall>> {
        Ok(self.call_with(calls, None)?.responses)
    }

    pub fn call_with(
        &self,
        calls: Vec<MethodCall>,
        created_ids: Option<&HashMap<String, String>>,
    ) -> CliResult<CallResponse> {
        let body = RequestBody {
            using: USING,
            method_calls: calls,
            created_ids,
        };
        let payload = serde_json::to_value(&body)?;
        let resp = self.client.post_json(self.api_path, &payload)?;
        let status = resp.status();
        if !status.is_success() {
            return Err(status_error(status));
        }
        let parsed: ResponseBody = resp.json()?;
        Ok(CallResponse {
            responses: parsed.method_responses,
            created_ids: parsed.created_ids,
        })
    }

    pub fn call(&self, method: &str, args: Value) -> CliResult<Value> {
        let responses = self.call_many(vec![(method.to_string(), args, "c0".to_string())])?;
        if responses.len() != 1 {
            return Err(CliError::UnexpectedResponse(format!(
                "expected 1 method response, got {}",
                responses.len()
            )));
        }
        let (name, result, _) = responses
            .into_iter()
            .next()
            .ok_or_else(|| CliError::UnexpectedResponse("empty method responses".into()))?;
        if name == "error" {
            return Err(jmap_error_from_value(&result));
        }
        Ok(result)
    }
}

pub fn jmap_error_from_value(result: &Value) -> CliError {
    let type_ = result
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("(unknown)")
        .to_string();
    let description = result
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    CliError::JmapMethodError { type_, description }
}

pub fn check_response(resp: MethodCall, expected_name: &str) -> CliResult<Value> {
    let (name, result, _) = resp;
    if name == "error" {
        return Err(jmap_error_from_value(&result));
    }
    if name != expected_name {
        return Err(CliError::UnexpectedResponse(format!(
            "expected response `{expected_name}`, got `{name}`"
        )));
    }
    Ok(result)
}
