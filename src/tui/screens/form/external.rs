/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::env;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const PREFIX: &str = "stalwart-cli-";
const SUFFIX: &str = ".json";
const NONCE_BYTES: usize = 8;

pub fn write(text: &str) -> io::Result<PathBuf> {
    let mut nonce = [0u8; NONCE_BYTES];
    getrandom::fill(&mut nonce).map_err(|err| io::Error::other(err.to_string()))?;
    let mut name = String::with_capacity(PREFIX.len() + NONCE_BYTES * 2 + SUFFIX.len());
    name.push_str(PREFIX);
    for byte in nonce {
        let _ = write!(name, "{byte:02x}");
    }
    name.push_str(SUFFIX);
    let path = env::temp_dir().join(name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&path)?;
    if let Err(err) = file.write_all(text.as_bytes()) {
        let _ = fs::remove_file(&path);
        return Err(err);
    }
    Ok(path)
}

pub fn take(path: &Path) -> io::Result<String> {
    let text = fs::read_to_string(path);
    let _ = fs::remove_file(path);
    text
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/form/external/tests.rs"]
mod tests;
