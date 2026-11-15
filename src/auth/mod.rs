/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod device;
pub mod discover;
pub mod error;
pub mod login;
pub mod register;
pub mod store;
pub mod token;

pub use error::AuthError;

pub const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
pub const FULL_SCOPE: &str = "urn:stalwart:oauth:scope:full";
pub const STALWART_SCOPE: &str = "openid offline_access urn:stalwart:oauth:scope:full";
