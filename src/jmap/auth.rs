/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::config::AuthMode;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

pub fn header_value(auth: &AuthMode) -> String {
    match auth {
        AuthMode::Basic { user, password } => {
            let mut raw = String::with_capacity(user.len() + 1 + password.len());
            raw.push_str(user);
            raw.push(':');
            raw.push_str(password);
            let mut out = String::from("Basic ");
            out.push_str(&STANDARD.encode(raw.as_bytes()));
            out
        }
        AuthMode::Bearer { token } => {
            let mut out = String::with_capacity(7 + token.len());
            out.push_str("Bearer ");
            out.push_str(token);
            out
        }
    }
}
