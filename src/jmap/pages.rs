/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::jmap::client::{CallScope, Client, Request, ResultRef};
use crate::jmap::method::MethodName;
use crate::jmap::response::{GetResponse, QueryResponse};
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};

pub(super) const QUERY_CALL_ID: &str = "q";
pub(super) const GET_CALL_ID: &str = "g";

#[derive(Clone, Copy)]
pub enum Fetch<'a> {
    Ids,
    Objects { properties: Option<&'a [&'a str]> },
}

pub struct Page {
    pub ids: Vec<String>,
    pub objects: Vec<Map<String, Value>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageState {
    More,
    Done,
}

pub struct Pager<'a> {
    filter: Option<&'a Value>,
    limit: usize,
    anchor: Option<String>,
    state: PageState,
}

pub struct QueryArgs<'a> {
    filter: Option<&'a Value>,
    limit: usize,
    anchor: Option<&'a str>,
}

pub struct Pages<'a> {
    client: &'a Client,
    scope: CallScope<'a>,
    object: &'a str,
    fetch: Fetch<'a>,
    pager: Pager<'a>,
}

#[derive(Serialize)]
pub(super) struct ChainedGet<'a> {
    #[serde(rename = "#ids")]
    pub(super) ids: ResultRef<'a>,
    pub(super) properties: Option<&'a [&'a str]>,
}

impl<'a> Pager<'a> {
    pub fn new(filter: Option<&'a Value>, limit: usize) -> Self {
        Pager {
            filter,
            limit,
            anchor: None,
            state: PageState::More,
        }
    }

    pub fn args(&self) -> QueryArgs<'_> {
        QueryArgs {
            filter: self.filter,
            limit: self.limit,
            anchor: self.anchor.as_deref(),
        }
    }

    pub fn advance(&mut self, query_ids: &[String]) -> PageState {
        self.state = match query_ids.last() {
            Some(last) if query_ids.len() >= self.limit => {
                self.anchor.get_or_insert_default().clone_from(last);
                PageState::More
            }
            _ => PageState::Done,
        };
        self.state
    }

    pub fn state(&self) -> PageState {
        self.state
    }

    fn finish(&mut self) {
        self.state = PageState::Done;
    }
}

impl Serialize for QueryArgs<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let len = 2 + usize::from(self.filter.is_some()) + usize::from(self.anchor.is_some());
        let mut map = serializer.serialize_map(Some(len))?;
        if let Some(filter) = self.filter {
            map.serialize_entry("filter", filter)?;
        }
        map.serialize_entry("limit", &self.limit)?;
        match self.anchor {
            None => map.serialize_entry("calculateTotal", &true)?,
            Some(anchor) => {
                map.serialize_entry("anchor", anchor)?;
                map.serialize_entry("anchorOffset", &1)?;
            }
        }
        map.end()
    }
}

impl<'a> Pages<'a> {
    pub(super) fn new(
        client: &'a Client,
        scope: CallScope<'a>,
        object: &'a str,
        filter: Option<&'a Value>,
        fetch: Fetch<'a>,
        limit: usize,
    ) -> Self {
        Pages {
            client,
            scope,
            object,
            fetch,
            pager: Pager::new(filter, limit),
        }
    }

    fn fetch_page(&self) -> Result<Page> {
        let query = MethodName::query(self.object);
        let args = self.pager.args();
        match self.fetch {
            Fetch::Ids => {
                let QueryResponse { ids, .. } = self.client.call_as(self.scope, query, &args)?;
                Ok(Page {
                    ids,
                    objects: Vec::new(),
                })
            }
            Fetch::Objects { properties } => {
                let get = MethodName::get(self.object);
                let chained = ChainedGet {
                    ids: ResultRef {
                        result_of: QUERY_CALL_ID,
                        name: query,
                        path: "/ids",
                    },
                    properties,
                };
                let mut request = Request::scoped(self.scope);
                request
                    .call(query, &args, QUERY_CALL_ID)?
                    .call(get, &chained, GET_CALL_ID)?;
                let mut response = self.client.send(&request)?;
                let QueryResponse { ids, .. } = response.take_as(QUERY_CALL_ID, query)?;
                let GetResponse { list } = response.take_as(GET_CALL_ID, get)?;
                Ok(Page { ids, objects: list })
            }
        }
    }
}

impl Iterator for Pages<'_> {
    type Item = Result<Page>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pager.state() == PageState::Done {
            return None;
        }
        match self.fetch_page() {
            Ok(page) => {
                self.pager.advance(&page.ids);
                (!page.ids.is_empty()).then_some(Ok(page))
            }
            Err(err) => {
                self.pager.finish();
                Some(Err(err))
            }
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/pages/tests.rs"]
mod tests;
