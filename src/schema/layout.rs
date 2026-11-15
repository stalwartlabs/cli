/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub items: Vec<LayoutItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum LayoutItem {
    Link { link: LayoutLink },
    Container { container: LayoutContainer },
    Unknown(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LayoutLink {
    pub name: String,
    #[serde(default)]
    pub icon: String,
    pub view_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LayoutContainer {
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub items: Vec<LayoutSubItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LayoutSubItem {
    #[serde(rename_all = "camelCase")]
    Link { name: String, view_name: String },
    #[serde(rename_all = "camelCase")]
    Container {
        name: String,
        #[serde(default)]
        items: Vec<LayoutSubItem>,
    },
    #[serde(other)]
    Unknown,
}
