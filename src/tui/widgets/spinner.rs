/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::time::Duration;

pub const FRAME: Duration = Duration::from_millis(100);
const FRAMES: [&str; 8] = ["⣷", "⣯", "⣟", "⡿", "⢿", "⣻", "⣽", "⣾"];

pub fn frame(tick: u64) -> &'static str {
    let len = FRAMES.len() as u64;
    FRAMES
        .get(usize::try_from(tick % len).unwrap_or(0))
        .copied()
        .unwrap_or(" ")
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/widgets/spinner/tests.rs"]
mod tests;
