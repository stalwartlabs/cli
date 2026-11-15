/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

mod dashboard;
pub mod defaults;
mod error;
pub mod expression;
#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/schema/fixture.rs"]
pub mod fixture;
mod layout;
pub mod object;
mod resolve;
#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/schema/test_support.rs"]
pub mod test_support;
mod types;
pub mod view;

pub use dashboard::*;
pub use error::{EmbeddedObject, ResolveError};
pub use layout::*;
pub use object::ObjectDef;
pub use types::*;
