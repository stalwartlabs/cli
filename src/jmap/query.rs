/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::jmap::client::{CallScope, Client, Request, ResultRef};
use crate::jmap::method::MethodName;
use crate::jmap::pages::{ChainedGet, GET_CALL_ID, QUERY_CALL_ID};
use crate::jmap::response::{GetResponse, QueryResponse};
use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sort<'a> {
    pub property: &'a str,
    #[serde(rename = "isAscending")]
    pub ascending: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Total {
    Calculate,
    Skip,
}

#[derive(Clone, Copy, Debug)]
pub struct PageQuery<'a> {
    pub filter: Option<&'a Value>,
    pub sort: Option<Sort<'a>>,
    pub position: i64,
    pub limit: usize,
    pub total: Total,
    pub properties: Option<&'a [&'a str]>,
}

#[derive(Debug, Default)]
pub struct PageResult {
    pub ids: Vec<String>,
    pub objects: Vec<Map<String, Value>>,
    pub total: Option<u64>,
}

struct QueryArgs<'q, 'a> {
    query: &'q PageQuery<'a>,
    limit: usize,
}

impl Client {
    pub fn query_page(
        &self,
        scope: CallScope<'_>,
        object: &str,
        query: &PageQuery<'_>,
    ) -> Result<PageResult> {
        let query_name = MethodName::query(object);
        let get_name = MethodName::get(object);
        let args = QueryArgs {
            query,
            limit: query.limit.clamp(1, self.max_get()),
        };
        let chained = ChainedGet {
            ids: ResultRef {
                result_of: QUERY_CALL_ID,
                name: query_name,
                path: "/ids",
            },
            properties: query.properties,
        };
        let mut request = Request::scoped(scope);
        request
            .call(query_name, &args, QUERY_CALL_ID)?
            .call(get_name, &chained, GET_CALL_ID)?;
        let mut response = self.send(&request)?;
        let QueryResponse { ids, total, .. } = response.take_as(QUERY_CALL_ID, query_name)?;
        let GetResponse { list } = response.take_as(GET_CALL_ID, get_name)?;
        Ok(PageResult {
            ids,
            objects: list,
            total,
        })
    }
}

impl Serialize for QueryArgs<'_, '_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let q = self.query;
        let mut map = serializer.serialize_map(None)?;
        if let Some(filter) = q.filter {
            map.serialize_entry("filter", filter)?;
        }
        if let Some(sort) = &q.sort {
            map.serialize_entry("sort", std::slice::from_ref(sort))?;
        }
        map.serialize_entry("position", &q.position)?;
        map.serialize_entry("limit", &self.limit)?;
        if q.total == Total::Calculate {
            map.serialize_entry("calculateTotal", &true)?;
        }
        map.end()
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/query/tests.rs"]
mod tests;
