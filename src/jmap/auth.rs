/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use encodify::base64::STANDARD;
use reqwest::header::HeaderValue;

const BASIC: &str = "Basic ";
const BEARER: &str = "Bearer ";

pub enum Auth {
    Basic { user: String, password: String },
    Bearer { token: String },
}

impl Auth {
    pub fn header(&self) -> Result<HeaderValue> {
        HeaderValue::try_from(self.header_text()).map_err(Error::InvalidAuthHeader)
    }

    pub fn bearer_header(token: &str) -> Result<HeaderValue> {
        let mut out = String::with_capacity(BEARER.len() + token.len());
        out.push_str(BEARER);
        out.push_str(token);
        HeaderValue::try_from(out).map_err(Error::InvalidAuthHeader)
    }

    fn header_text(&self) -> String {
        match self {
            Auth::Basic { user, password } => {
                let mut raw = String::with_capacity(user.len() + 1 + password.len());
                raw.push_str(user);
                raw.push(':');
                raw.push_str(password);
                let mut out = String::with_capacity(BASIC.len() + STANDARD.encoded_len(raw.len()));
                out.push_str(BASIC);
                STANDARD.encode_append(raw, &mut out);
                out
            }
            Auth::Bearer { token } => {
                let mut out = String::with_capacity(BEARER.len() + token.len());
                out.push_str(BEARER);
                out.push_str(token);
                out
            }
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/auth/tests.rs"]
mod tests;
