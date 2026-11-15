/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::jmap::Client;
use crate::jmap::blob::BlobUpload;
use crate::jmap::client::{CallScope, Request};
use crate::jmap::method::MethodName;
use crate::jmap::pages::Fetch;
use crate::jmap::query::{PageQuery, PageResult, Sort, Total};
use crate::jmap::response::SetResponse;
use crate::plan::bulk::{self, BulkSummary, BulkTarget};
use crate::schema::{Filter, Schema};
use crate::tui::exec::{Cancel, JobCx};
use crate::tui::job::tools::{self, ApplySpec, SnapshotSpec, ToolReport};
use crate::tui::msg::JobProgress;
use crate::view::labels::{LabelRequest, Labels};
use serde_json::{Map, Value};
use std::ops::ControlFlow;
use std::sync::Arc;

pub const PICKER_FULL_LIMIT: usize = 5000;
const MAX_PICKER_SETS: usize = 16;
const BLOB_MEDIA_TYPE: &str = "text/plain";
const SET_CALL_ID: &str = "c0";
const NO_ACCOUNT: &str = "the session has no primary account for blobs";

pub type Reply<T> = Result<Box<T>, Box<Error>>;
pub type BlobTexts = Vec<(Box<str>, Option<String>)>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ObjectsSlot {
    Page(u32),
    Labels(u32),
    Load,
    Save,
    Picker,
}

pub struct ObjectsJob {
    client: Arc<Client>,
    schema: Arc<Schema>,
    account: Option<Box<str>>,
    kind: ObjectsKind,
}

pub enum ObjectsKind {
    Page(Box<PageSpec>),
    Labels {
        position: i64,
        request: Box<LabelRequest>,
    },
    Get(Box<GetSpec>),
    Set(Box<SetSpec>),
    PickerOptions(Box<PickerSpec>),
    BlobTexts(Box<BlobSpec>),
    Bulk(Box<BulkSpec>),
    Apply(Box<ApplySpec>),
    Snapshot(Box<SnapshotSpec>),
}

pub struct PageSpec {
    pub object: Box<str>,
    pub filter: Option<Value>,
    pub sort: Option<(Box<str>, bool)>,
    pub position: i64,
    pub limit: usize,
    pub total: Total,
    pub properties: Box<[Box<str>]>,
}

pub struct GetSpec {
    pub object: Box<str>,
    pub id: Box<str>,
    pub properties: Option<Box<[Box<str>]>>,
}

pub struct SetSpec {
    pub object: Box<str>,
    pub args: Value,
    pub uploads: Vec<Upload>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    pub creation_id: Box<str>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BulkSelection {
    Ids(Vec<String>),
    Matching(Option<Value>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BulkAction {
    Destroy,
    SetProperty(Map<String, Value>),
}

pub struct BulkSpec {
    pub object: Box<str>,
    pub selection: BulkSelection,
    pub action: BulkAction,
    pub cancel: Cancel,
}

pub struct BlobSpec {
    pub blobs: Vec<(Box<str>, Box<str>)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickerSpec {
    pub object: Box<str>,
    pub label: Option<Box<str>>,
    pub filter: Option<Value>,
    pub search_field: Option<Box<str>>,
    pub search: Option<Box<str>>,
}

pub const TENANT_PROPERTY: &str = "memberTenantId";

impl PickerSpec {
    pub fn new(schema: &Schema, object: &str, tenant: Option<&str>) -> Self {
        let view = schema.view(object);
        let list = view
            .and_then(|view| view.list)
            .or_else(|| schema.lists.get(object));
        let statics = list
            .map(|list| &list.filters_static)
            .filter(|filter| !filter.is_empty());
        let label = view.and_then(|view| view.label_property()).map(Box::from);
        let object = view.map_or(object, |view| view.object.name());
        let tenant = tenant.filter(|_| {
            schema
                .get_object(object)
                .is_some_and(|def| def.all_fields().get(TENANT_PROPERTY).is_some())
        });
        let filter = (statics.is_some() || tenant.is_some()).then(|| {
            let mut filter: Map<String, Value> = statics
                .into_iter()
                .flatten()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            if let Some(tenant) = tenant {
                filter.insert(TENANT_PROPERTY.into(), Value::String(tenant.into()));
            }
            Value::Object(filter)
        });
        let search_field = list
            .and_then(|list| {
                list.filters.iter().find_map(|filter| match filter {
                    Filter::Text { field, .. } => Some(field.as_str()),
                    _ => None,
                })
            })
            .map(Box::from);
        PickerSpec {
            object: object.into(),
            label,
            filter,
            search_field,
            search: None,
        }
    }

    pub fn searching(&self, query: &str) -> Self {
        PickerSpec {
            search: (!query.is_empty()).then(|| query.into()),
            ..self.clone()
        }
    }
}

#[derive(Default)]
pub struct PickerCache {
    entries: Vec<PickerOptions>,
}

impl PickerCache {
    pub fn get(&mut self, spec: &PickerSpec) -> Option<&PickerOptions> {
        let pos = self.entries.iter().position(|entry| entry.spec == *spec)?;
        let entry = self.entries.remove(pos);
        self.entries.push(entry);
        self.entries.last()
    }

    pub fn put(&mut self, options: PickerOptions) {
        if options.spec.search.is_some() {
            return;
        }
        self.entries.retain(|entry| entry.spec != options.spec);
        if self.entries.len() >= MAX_PICKER_SETS {
            self.entries.remove(0);
        }
        self.entries.push(options);
    }

    pub fn forget(&mut self, object: &str) {
        self.entries.retain(|entry| &*entry.spec.object != object);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PickerOptions {
    pub spec: PickerSpec,
    pub options: Vec<(Box<str>, Box<str>)>,
    pub truncated: bool,
}

pub enum ObjectsOutput {
    Page {
        position: i64,
        result: Reply<PageResult>,
    },
    Labels {
        position: i64,
        labels: Box<Labels>,
    },
    Get(Reply<Option<Map<String, Value>>>),
    Set(Reply<SetResponse>),
    PickerOptions(Reply<PickerOptions>),
    BlobTexts(Reply<BlobTexts>),
    Bulk(Reply<BulkSummary>),
    Tool(Box<ToolReport>),
}

impl ObjectsOutput {
    pub fn error(&self) -> Option<&Error> {
        match self {
            ObjectsOutput::Page { result, .. } => result.as_ref().err(),
            ObjectsOutput::Get(reply) => reply.as_ref().err(),
            ObjectsOutput::Set(reply) => reply.as_ref().err(),
            ObjectsOutput::PickerOptions(reply) => reply.as_ref().err(),
            ObjectsOutput::BlobTexts(reply) => reply.as_ref().err(),
            ObjectsOutput::Bulk(Ok(summary)) => return summary.error(),
            ObjectsOutput::Bulk(Err(err)) => Some(err),
            ObjectsOutput::Labels { .. } | ObjectsOutput::Tool(_) => None,
        }
        .map(AsRef::as_ref)
    }
}

impl ObjectsJob {
    pub fn is_read_only(&self) -> bool {
        match self.kind {
            ObjectsKind::Page(_)
            | ObjectsKind::Labels { .. }
            | ObjectsKind::Get(_)
            | ObjectsKind::PickerOptions(_)
            | ObjectsKind::BlobTexts(_) => true,
            ObjectsKind::Set(_)
            | ObjectsKind::Bulk(_)
            | ObjectsKind::Apply(_)
            | ObjectsKind::Snapshot(_) => false,
        }
    }

    pub fn new(
        client: Arc<Client>,
        schema: Arc<Schema>,
        account: Option<Box<str>>,
        kind: ObjectsKind,
    ) -> Self {
        Self {
            client,
            schema,
            account,
            kind,
        }
    }

    pub fn run(self, cx: &JobCx) -> ObjectsOutput {
        let Self {
            client,
            schema,
            account,
            kind,
        } = self;
        let scope = account
            .as_deref()
            .map_or(CallScope::Stalwart, CallScope::Account);
        match kind {
            ObjectsKind::Page(spec) => ObjectsOutput::Page {
                position: spec.position,
                result: boxed(page(&client, scope, &spec)),
            },
            ObjectsKind::Labels { position, request } => ObjectsOutput::Labels {
                position,
                labels: Box::new(request.fetch(&client, scope)),
            },
            ObjectsKind::Get(spec) => ObjectsOutput::Get(boxed(get(&client, scope, &spec))),
            ObjectsKind::Set(spec) if spec.uploads.is_empty() => {
                ObjectsOutput::Set(boxed(client.set_in(scope, &spec.object, &spec.args)))
            }
            ObjectsKind::Set(spec) => {
                ObjectsOutput::Set(boxed(set_with_uploads(&client, account.as_deref(), &spec)))
            }
            ObjectsKind::Bulk(spec) => {
                ObjectsOutput::Bulk(boxed(bulk_run(&client, scope, &spec, cx)))
            }
            ObjectsKind::BlobTexts(spec) => {
                ObjectsOutput::BlobTexts(boxed(blob_texts(&client, account.as_deref(), &spec)))
            }
            ObjectsKind::PickerOptions(spec) => {
                ObjectsOutput::PickerOptions(boxed(picker_options(&client, &schema, scope, *spec)))
            }
            ObjectsKind::Apply(spec) => {
                ObjectsOutput::Tool(Box::new(tools::apply(&client, &schema, &spec, cx)))
            }
            ObjectsKind::Snapshot(spec) => {
                ObjectsOutput::Tool(Box::new(tools::snapshot(&client, &schema, &spec, cx)))
            }
        }
    }
}

fn set_with_uploads(client: &Client, account: Option<&str>, spec: &SetSpec) -> Result<SetResponse> {
    let account = account.ok_or_else(no_account)?;
    let uploads: Vec<BlobUpload<'_>> = spec
        .uploads
        .iter()
        .map(|upload| BlobUpload {
            creation_id: &upload.creation_id,
            text: &upload.text,
            media_type: BLOB_MEDIA_TYPE,
        })
        .collect();
    let mut request = Request::scoped(CallScope::Account(account));
    request
        .upload_text(&uploads)?
        .call(MethodName::set(&spec.object), &spec.args, SET_CALL_ID)?;
    let mut response = client.send(&request)?;
    response.take_as(SET_CALL_ID, MethodName::set(&spec.object))
}

fn blob_texts(client: &Client, account: Option<&str>, spec: &BlobSpec) -> Result<BlobTexts> {
    let account = account.ok_or_else(no_account)?;
    spec.blobs
        .iter()
        .map(|(name, id)| Ok((name.clone(), client.blob_text(account, id)?)))
        .collect()
}

fn bulk_run(
    client: &Client,
    scope: CallScope<'_>,
    spec: &BulkSpec,
    cx: &JobCx,
) -> Result<BulkSummary> {
    let target = match &spec.selection {
        BulkSelection::Ids(ids) => BulkTarget::Ids(ids),
        BulkSelection::Matching(filter) => BulkTarget::Matching(filter.as_ref()),
    };
    let on = &mut |event| {
        cx.report(JobProgress::Bulk(event));
        if spec.cancel.is_cancelled() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    Ok(match &spec.action {
        BulkAction::Destroy => bulk::destroy(client, scope, &spec.object, target, on),
        BulkAction::SetProperty(patch) => {
            bulk::set_property(client, scope, &spec.object, target, patch, on)
        }
    })
}

#[cold]
fn no_account() -> Error {
    Error::UnexpectedResponse(NO_ACCOUNT.into())
}

fn boxed<T>(result: Result<T>) -> Reply<T> {
    result.map(Box::new).map_err(Box::new)
}

fn page(client: &Client, scope: CallScope<'_>, spec: &PageSpec) -> Result<PageResult> {
    let properties: Vec<&str> = spec.properties.iter().map(AsRef::as_ref).collect();
    let query = PageQuery {
        filter: spec.filter.as_ref(),
        sort: spec.sort.as_ref().map(|(property, ascending)| Sort {
            property,
            ascending: *ascending,
        }),
        position: spec.position,
        limit: spec.limit,
        total: spec.total,
        properties: (!properties.is_empty()).then_some(properties.as_slice()),
    };
    client.query_page(scope, &spec.object, &query)
}

fn get(
    client: &Client,
    scope: CallScope<'_>,
    spec: &GetSpec,
) -> Result<Option<Map<String, Value>>> {
    let properties: Option<Vec<&str>> = spec
        .properties
        .as_ref()
        .map(|list| list.iter().map(AsRef::as_ref).collect());
    let response = client.get_in(scope, &spec.object, &[&spec.id], properties.as_deref())?;
    Ok(response.list.into_iter().next())
}

fn picker_options(
    client: &Client,
    schema: &Schema,
    scope: CallScope<'_>,
    spec: PickerSpec,
) -> Result<PickerOptions> {
    let label = spec.label.as_deref().or_else(|| {
        schema
            .get_object(&spec.object)
            .and_then(|def| def.label_property())
    });
    let properties = [label.unwrap_or("id")];
    let searched;
    let filter = match (&spec.search_field, &spec.search) {
        (Some(field), Some(text)) => {
            let mut map = match &spec.filter {
                Some(Value::Object(map)) => map.clone(),
                _ => Map::with_capacity(1),
            };
            map.insert(field.to_string(), Value::String(text.to_string()));
            searched = Value::Object(map);
            Some(&searched)
        }
        _ => spec.filter.as_ref(),
    };
    let mut options = Vec::new();
    let mut truncated = false;
    'pages: for page in client.pages_in(
        scope,
        &spec.object,
        filter,
        Fetch::Objects {
            properties: Some(&properties),
        },
    ) {
        for mut object in page?.objects {
            if options.len() >= PICKER_FULL_LIMIT {
                truncated = true;
                break 'pages;
            }
            let Some(Value::String(id)) = object.remove("id") else {
                continue;
            };
            let text = match label.and_then(|name| object.remove(name)) {
                Some(Value::String(text)) if !text.is_empty() => text,
                _ => id.clone(),
            };
            options.push((id.into_boxed_str(), text.into_boxed_str()));
        }
    }
    Ok(PickerOptions {
        spec,
        options,
        truncated,
    })
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/job/objects/tests.rs"]
mod tests;

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/job/objects/test_support.rs"]
mod test_support;
