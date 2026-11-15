/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::token;
use crate::cli::args::GlobalArgs;
use crate::cli::config::{resolve_url, tls_of};
use crate::cli::login::token_store;
use crate::cli::message::CliMessage;
use crate::error::Result;
use crate::jmap::http::HttpClient;
use std::io::{self, Write};

pub fn run(global: &GlobalArgs) -> Result<()> {
    let url = resolve_url(global)?;
    let mut err = io::stderr().lock();
    let user = global.user.as_deref();
    let Some(store) = token_store(&url) else {
        writeln!(err, "Not signed in to {url}.")?;
        return Ok(());
    };
    let Some(account) = store.signed_in(user) else {
        match (user, store.last_account()) {
            (Some(user), Some(last)) => writeln!(
                err,
                "Not signed in to {url} as {user} (last signed in as {last}; use --user {last})."
            )?,
            (Some(user), None) => writeln!(err, "Not signed in to {url} as {user}.")?,
            (None, _) => writeln!(err, "Not signed in to {url}.")?,
        }
        return Ok(());
    };
    let tls = tls_of(global);
    let revoked = store.sign_out(&account, |session| {
        let session = session?;
        let endpoint = session.revocation_endpoint.as_deref()?;
        let token = session
            .refresh_token
            .as_deref()
            .unwrap_or(&session.access_token);
        HttpClient::anonymous(&url, tls)
            .and_then(|http| token::revoke(&http, endpoint, &session.client_id, token))
            .err()
    })?;
    if let Some(e) = revoked {
        writeln!(
            err,
            "warning: could not revoke the token: {}",
            CliMessage(&e)
        )?;
    }
    writeln!(err, "Signed out {account}.")?;
    Ok(())
}
