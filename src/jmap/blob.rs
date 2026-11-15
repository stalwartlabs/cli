/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::jmap::client::{CallScope, Client, Request};
use crate::jmap::method::{Method, MethodName};
use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use serde_json::Value;

const BLOB: &str = "Blob";
const AS_TEXT: &str = "data:asText";
pub const UPLOAD_CALL_ID: &str = "upload";
pub const UPLOAD: MethodName<'static> = MethodName::new(BLOB, Method::Upload);

#[derive(Clone, Copy, Debug)]
pub struct BlobUpload<'a> {
    pub creation_id: &'a str,
    pub text: &'a str,
    pub media_type: &'a str,
}

struct UploadArgs<'r, 'a>(&'r [BlobUpload<'a>]);

#[derive(Serialize)]
struct UploadObject<'a> {
    data: [TextSource<'a>; 1],
    #[serde(rename = "type")]
    media_type: &'a str,
}

#[derive(Serialize)]
struct TextSource<'a> {
    #[serde(rename = "data:asText")]
    text: &'a str,
}

#[derive(Serialize)]
struct CreateMap<'r, 'a> {
    create: UploadArgs<'r, 'a>,
}

impl Serialize for UploadArgs<'_, '_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for upload in self.0 {
            map.serialize_entry(
                upload.creation_id,
                &UploadObject {
                    data: [TextSource { text: upload.text }],
                    media_type: upload.media_type,
                },
            )?;
        }
        map.end()
    }
}

impl Client {
    pub fn blob_text(&self, account: &str, blob_id: &str) -> Result<Option<String>> {
        let mut response = self.get_in(
            CallScope::Account(account),
            BLOB,
            &[blob_id],
            Some(&[AS_TEXT]),
        )?;
        Ok(response
            .list
            .iter_mut()
            .find(|blob| blob.get("id").and_then(Value::as_str) == Some(blob_id))
            .and_then(|blob| match blob.remove(AS_TEXT) {
                Some(Value::String(text)) => Some(text),
                _ => None,
            }))
    }
}

impl<'a> Request<'a> {
    pub fn upload_text(&mut self, uploads: &[BlobUpload<'a>]) -> Result<&mut Self> {
        self.call(
            UPLOAD,
            &CreateMap {
                create: UploadArgs(uploads),
            },
            UPLOAD_CALL_ID,
        )
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/jmap/blob/tests.rs"]
mod tests;
