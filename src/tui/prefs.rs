/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::theme::Palette;
use crate::tui::widgets::sidebar::SidebarMode;
use crate::view::metrics::RangePreset;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const MAX_PREFS_BYTES: u64 = 1 << 20;

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Palette>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar: Option<SidebarMode>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub last_view: BTreeMap<Box<str>, Box<str>>,
    #[serde(default, skip_serializing_if = "Period::is_default")]
    pub period: Period,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Period {
    Preset { preset: RangePreset },
    Custom { from: i64, to: i64 },
}

impl Default for Period {
    fn default() -> Self {
        Period::Preset {
            preset: RangePreset::default(),
        }
    }
}

impl Period {
    fn is_default(&self) -> bool {
        *self == Period::default()
    }
}

impl Prefs {
    pub fn load(path: &Path) -> Self {
        Self::read(path).unwrap_or_default()
    }

    fn read(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        if file.metadata().ok()?.len() > MAX_PREFS_BYTES {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(MAX_PREFS_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if u64::try_from(bytes.len()).ok()? > MAX_PREFS_BYTES {
            return None;
        }
        serde_json::from_slice(&bytes).ok()
    }

    pub fn last_view(&self, layout: &str) -> Option<&str> {
        self.last_view.get(layout).map(AsRef::as_ref)
    }

    pub fn set_period(&mut self, period: Period) -> bool {
        if self.period == period {
            return false;
        }
        self.period = period;
        true
    }

    pub fn remember(&mut self, layout: &str, view: &str) -> bool {
        if self.last_view(layout) == Some(view) {
            return false;
        }
        self.last_view.insert(layout.into(), view.into());
        true
    }

    pub fn save(&self, path: &Path, writer_id: u32) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        let tmp = tmp_path(path, writer_id);
        if let Err(err) = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path)) {
            let _ = fs::remove_file(&tmp);
            return Err(err);
        }
        Ok(())
    }
}

fn tmp_path(path: &Path, writer_id: u32) -> PathBuf {
    let mut name = path.file_name().map_or_else(OsString::new, OsString::from);
    name.push(".tmp.");
    name.push(writer_id.to_string());
    path.with_file_name(name)
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/tui/prefs/tests.rs"]
mod tests;
