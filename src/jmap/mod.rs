/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod account;
pub mod auth;
pub mod blob;
pub mod cache;
pub mod client;
pub mod credential;
pub mod http;
pub mod live;
pub mod method;
pub mod pages;
pub mod query;
pub mod response;
pub mod schema_fetch;
pub mod session;
pub mod set_error;
pub mod sse;
#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/test_support.rs"]
pub mod test_support;

pub use client::Client;
