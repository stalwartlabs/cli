/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::set_error::SetError;
use serde::de::{DeserializeOwned, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Deserialize)]
pub struct QueryResponse {
    pub ids: Vec<String>,
    #[serde(default)]
    pub total: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct GetResponse {
    pub list: Vec<Map<String, Value>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetResponse {
    #[serde(default, deserialize_with = "null_as_default")]
    pub created: Map<String, Value>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub not_created: SetFailures,
    #[serde(default, deserialize_with = "null_as_default")]
    pub updated: HashMap<String, Option<Value>>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub not_updated: SetFailures,
    #[serde(default, deserialize_with = "null_as_default")]
    pub destroyed: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub not_destroyed: SetFailures,
}

#[derive(Debug, Default)]
pub struct SetFailures(Vec<(String, SetError)>);

impl SetFailures {
    pub fn iter(&self) -> impl Iterator<Item = (&str, &SetError)> {
        self.0.iter().map(|(id, err)| (id.as_str(), err))
    }

    pub fn first(&self) -> Option<(&str, &SetError)> {
        self.0.first().map(|(id, err)| (id.as_str(), err))
    }
}

impl IntoIterator for SetFailures {
    type Item = (String, SetError);
    type IntoIter = std::vec::IntoIter<(String, SetError)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'de> Deserialize<'de> for SetFailures {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FailuresVisitor;

        impl<'de> Visitor<'de> for FailuresVisitor {
            type Value = SetFailures;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a map of id to SetError")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<SetFailures, A::Error> {
                let mut out = Vec::new();
                while let Some(entry) = map.next_entry::<String, SetError>()? {
                    out.push(entry);
                }
                Ok(SetFailures(out))
            }
        }

        deserializer.deserialize_map(FailuresVisitor)
    }
}

fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/response/tests.rs"]
mod tests;
