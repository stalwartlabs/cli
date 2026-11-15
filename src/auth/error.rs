/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("server discovery failed (HTTP {0})")]
    DiscoveryFailed(u16),
    #[error("client registration refused: {0}")]
    RegistrationRefused(String),
    #[error("the OAuth client is not allowed to use the device flow")]
    UnauthorizedClient,
    #[error("the OAuth client is unknown to the server")]
    InvalidClient,
    #[error("the requested scope was refused")]
    InvalidScope,
    #[error("the sign-in request was denied")]
    AccessDenied,
    #[error("the sign-in code expired")]
    Expired,
    #[error("identity provider issuer mismatch: expected `{expected}`, got `{got}`")]
    IssuerMismatch { expected: String, got: String },
    #[error("the identity provider does not support the device flow")]
    NoDeviceEndpoint,
    #[error("an OAuth client id registered with the identity provider is required")]
    MissingClientId,
    #[error("OAuth error: {error}{}", description.as_deref().map(|d| format!(" ({d})")).unwrap_or_default())]
    OAuth {
        error: String,
        description: Option<String>,
    },
}
