/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::token::OAuthErrorBody;
use crate::auth::{AuthError, DEVICE_CODE_GRANT, STALWART_SCOPE};
use crate::error::Result;
use crate::jmap::http::HttpClient;
use serde::{Deserialize, Serialize};

pub const CLIENT_NAME: &str = "Stalwart CLI";
const LOOPBACK_REDIRECT: &str = "http://127.0.0.1/";

#[derive(Serialize)]
struct Registration<'a> {
    client_name: &'a str,
    redirect_uris: [&'a str; 1],
    grant_types: [&'a str; 3],
    response_types: [&'a str; 1],
    token_endpoint_auth_method: &'a str,
    scope: &'a str,
}

#[derive(Deserialize)]
struct Registered {
    client_id: String,
}

pub fn register_device_client(
    http: &HttpClient,
    registration_endpoint: &str,
    client_name: &str,
) -> Result<String> {
    let body = serde_json::to_vec(&Registration {
        client_name,
        redirect_uris: [LOOPBACK_REDIRECT],
        grant_types: ["authorization_code", "refresh_token", DEVICE_CODE_GRANT],
        response_types: ["code"],
        token_endpoint_auth_method: "none",
        scope: STALWART_SCOPE,
    })?;
    let response = http.post_json_anonymous(registration_endpoint, body)?;
    let status = response.status();
    if status.is_success() {
        return Ok(response.json::<Registered>()?.client_id);
    }
    let reason = match response.json::<OAuthErrorBody>() {
        Ok(OAuthErrorBody {
            error,
            error_description: Some(description),
        }) => format!("{error}: {description}"),
        Ok(body) => body.error,
        Err(_) => format!("HTTP {}", status.as_u16()),
    };
    Err(AuthError::RegistrationRefused(reason).into())
}
