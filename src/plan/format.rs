/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::plan::error::PlanError;
use crate::plan::op::OpKind;
use crate::schema::object::{display_name, split_view};
use serde::Deserialize;
use serde::de::{Deserializer, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt::{self, Write as _};
use std::io::Write;

#[derive(Debug, Clone)]
pub enum RawMatchOn {
    Wildcard(String),
    Props(Vec<String>),
}

impl<'de> Deserialize<'de> for RawMatchOn {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MatchOnVisitor;

        impl<'de> Visitor<'de> for MatchOnVisitor {
            type Value = RawMatchOn;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a list of property names or the string \"*\"")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<RawMatchOn, E> {
                Ok(RawMatchOn::Wildcard(value.to_string()))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<RawMatchOn, A::Error> {
                let mut props = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(16));
                while let Some(prop) = seq.next_element::<String>()? {
                    props.push(prop);
                }
                Ok(RawMatchOn::Props(props))
            }
        }

        deserializer.deserialize_any(MatchOnVisitor)
    }
}

#[derive(Deserialize, Debug)]
#[serde(tag = "@type", rename_all = "lowercase", deny_unknown_fields)]
pub enum RawOp {
    Update {
        object: String,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        scope: Option<Value>,
        #[serde(default)]
        value: Value,
    },
    Destroy {
        object: String,
        #[serde(default)]
        scope: Option<Value>,
        #[serde(default)]
        value: Option<Value>,
    },
    Create {
        object: String,
        #[serde(default)]
        scope: Option<Value>,
        #[serde(default)]
        value: Map<String, Value>,
    },
    Upsert {
        object: String,
        #[serde(default, rename = "matchOn")]
        match_on: Option<RawMatchOn>,
        #[serde(default)]
        scope: Option<Value>,
        #[serde(default)]
        value: Map<String, Value>,
    },
    Reconcile {
        object: String,
        #[serde(default, rename = "matchOn")]
        match_on: Option<RawMatchOn>,
        #[serde(default)]
        scope: Option<Value>,
        #[serde(default)]
        value: Map<String, Value>,
    },
}

impl RawOp {
    pub fn kind(&self) -> OpKind {
        match self {
            RawOp::Update { .. } => OpKind::Update,
            RawOp::Destroy { .. } => OpKind::Destroy,
            RawOp::Create { .. } => OpKind::Create,
            RawOp::Upsert { .. } => OpKind::Upsert,
            RawOp::Reconcile { .. } => OpKind::Reconcile,
        }
    }

    pub fn object(&self) -> &str {
        match self {
            RawOp::Update { object, .. }
            | RawOp::Destroy { object, .. }
            | RawOp::Create { object, .. }
            | RawOp::Upsert { object, .. }
            | RawOp::Reconcile { object, .. } => object,
        }
    }
}

pub fn parse_ndjson(input: &str) -> Result<Vec<RawOp>> {
    let mut ops = Vec::new();
    for (index, line) in input.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let op = serde_json::from_str(trimmed).map_err(|source| PlanError::Syntax {
            line: index + 1,
            source,
        })?;
        ops.push(op);
    }
    Ok(ops)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanRef<'a> {
    pub prefix: &'a str,
    pub id: &'a str,
}

impl PlanRef<'_> {
    pub fn prefix(canonical: &str) -> String {
        split_view(display_name(canonical)).0.to_ascii_lowercase()
    }

    pub fn client_id(raw: &str) -> Option<&str> {
        raw.strip_prefix('#').filter(|s| !s.is_empty())
    }

    pub fn client_id_of(raw: &Value) -> Option<&str> {
        raw.as_str().and_then(PlanRef::client_id)
    }

    pub fn reference(self) -> String {
        let mut out = String::with_capacity(self.prefix.len() + self.id.len() + 2);
        out.push('#');
        let _ = write!(out, "{self}");
        out
    }
}

impl fmt::Display for PlanRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.prefix)?;
        f.write_str("-")?;
        f.write_str(self.id)
    }
}

#[derive(Clone, Copy)]
enum Entries {
    None,
    Some,
}

pub struct PlanWriter<'w> {
    out: &'w mut dyn Write,
    entries: Entries,
}

impl<'w> PlanWriter<'w> {
    pub fn new(out: &'w mut dyn Write) -> Self {
        PlanWriter {
            out,
            entries: Entries::None,
        }
    }

    pub fn flush(&mut self) -> Result<()> {
        Ok(self.out.flush()?)
    }

    pub fn begin_upsert(&mut self, object: &str, match_on: Option<&str>) -> Result<()> {
        self.out.write_all(b"{\"@type\":\"upsert\",\"object\":\"")?;
        self.out.write_all(object.as_bytes())?;
        self.out.write_all(b"\"")?;
        if let Some(key) = match_on {
            self.out.write_all(b",\"matchOn\":[")?;
            serde_json::to_writer(&mut *self.out, key)?;
            self.out.write_all(b"]")?;
        }
        self.out.write_all(b",\"value\":{")?;
        self.entries = Entries::None;
        Ok(())
    }

    pub fn entry(&mut self, key: PlanRef<'_>, body: &Map<String, Value>) -> Result<()> {
        if matches!(self.entries, Entries::Some) {
            self.out.write_all(b",")?;
        }
        self.entries = Entries::Some;
        self.out.write_all(b"\"")?;
        write!(self.out, "{key}")?;
        self.out.write_all(b"\":")?;
        serde_json::to_writer(&mut *self.out, body)?;
        Ok(())
    }

    pub fn end_upsert(&mut self) -> Result<()> {
        Ok(self.out.write_all(b"}}\n")?)
    }

    pub fn update(
        &mut self,
        object: &str,
        id: Option<PlanRef<'_>>,
        body: &Map<String, Value>,
    ) -> Result<()> {
        self.out.write_all(b"{\"@type\":\"update\",\"object\":\"")?;
        self.out.write_all(object.as_bytes())?;
        if let Some(id) = id {
            self.out.write_all(b"\",\"id\":\"#")?;
            write!(self.out, "{id}")?;
        }
        self.out.write_all(b"\",\"value\":")?;
        serde_json::to_writer(&mut *self.out, body)?;
        Ok(self.out.write_all(b"}\n")?)
    }
}
