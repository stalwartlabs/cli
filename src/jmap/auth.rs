/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::config::AuthMode;
use encodify::base64::STANDARD;

pub fn header_value(auth: &AuthMode) -> String {
    match auth {
        AuthMode::Basic { user, password } => {
            let mut raw = String::with_capacity(user.len() + 1 + password.len());
            raw.push_str(user);
            raw.push(':');
            raw.push_str(password);
            let mut out = String::with_capacity(6 + STANDARD.encoded_len(raw.len()));
            out.push_str("Basic ");
            STANDARD.encode_append(raw, &mut out);
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
