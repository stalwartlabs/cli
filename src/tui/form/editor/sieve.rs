/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use encodify::base64::URL_SAFE_NO_PAD;
use flate2::Compression;
use flate2::write::DeflateEncoder;
use serde::Serialize;
use std::io::{self, Write};

pub const SIEVEPAD_URL: &str = "https://sievepad.com/";
const FORMAT_VERSION: u8 = 1;
const MAX_NAME_CHARS: usize = 80;
const MAIN_SCRIPT: &str = "main";

#[derive(Serialize)]
struct Workspace<'a> {
    v: u8,
    name: &'a str,
    scripts: [Script<'a>; 1],
    messages: [(); 0],
    settings: Settings,
}

#[derive(Serialize)]
struct Script<'a> {
    name: &'a str,
    source: &'a str,
}

#[derive(Serialize)]
struct Settings {}

pub fn sievepad_link(name: &str, source: &str) -> io::Result<String> {
    let name = name
        .char_indices()
        .nth(MAX_NAME_CHARS)
        .and_then(|(end, _)| name.get(..end))
        .unwrap_or(name);
    let source = source.replace("\r\n", "\n");
    let json = serde_json::to_vec(&Workspace {
        v: FORMAT_VERSION,
        name,
        scripts: [Script {
            name: MAIN_SCRIPT,
            source: &source,
        }],
        messages: [],
        settings: Settings {},
    })?;
    let mut encoder = DeflateEncoder::new(Vec::with_capacity(json.len() / 2), Compression::best());
    encoder.write_all(&json)?;
    let packed = encoder.finish()?;
    let mut link = String::with_capacity(SIEVEPAD_URL.len() + 3 + packed.len() * 4 / 3 + 4);
    link.push_str(SIEVEPAD_URL);
    link.push_str("#w=");
    link.push_str(&URL_SAFE_NO_PAD.encode(&packed));
    Ok(link)
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/form/editor/sieve/tests.rs"]
mod tests;
