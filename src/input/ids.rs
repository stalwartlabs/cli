/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::input::InputError;
use serde_json::Value;

pub fn parse_ids(raw: &str) -> Result<Vec<String>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(trimmed) {
        return items
            .into_iter()
            .map(|item| match item {
                Value::String(id) => Ok(id),
                _ => Err(InputError::IdsNotStrings.into()),
            })
            .collect();
    }
    Ok(trimmed
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect())
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/input/ids/tests.rs"]
mod tests;
