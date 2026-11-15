/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::plan::AT_TYPE;
use crate::plan::error::{ApplyError, OpRef};
use crate::plan::op::MatchOn;
use crate::plan::refs::{ClientIdRegistry, ResolvedScope};
use crate::plan::store::{Object, object_id};
use crate::schema::{Fields, ObjectDef};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::mem;

#[derive(Debug, Clone, Copy)]
pub enum Props<'a> {
    Listed(&'a [String]),
    Label(&'a str),
}

impl<'a> Props<'a> {
    pub fn iter(self) -> impl Iterator<Item = &'a str> {
        let (listed, label) = match self {
            Props::Listed(listed) => (listed, None),
            Props::Label(label) => (&[][..], Some(label)),
        };
        listed.iter().map(String::as_str).chain(label)
    }

    fn joined(self) -> String {
        let mut out = String::new();
        for (i, p) in self.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(p);
        }
        out
    }
}

#[derive(Debug, Clone, Copy)]
pub enum MatchKey<'a> {
    Props(Props<'a>),
    Value,
}

impl<'a> MatchKey<'a> {
    pub fn resolve(def: ObjectDef<'a>, match_on: Option<&'a MatchOn>) -> Self {
        match match_on {
            Some(MatchOn::Props(keys)) => MatchKey::Props(Props::Listed(keys)),
            Some(MatchOn::Wildcard) => MatchKey::Value,
            None => match def.label_property() {
                Some(label) => MatchKey::Props(Props::Label(label)),
                None => MatchKey::Value,
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct Signature(String);

impl Signature {
    fn clear(&mut self) {
        self.0.clear();
    }

    fn columns(&mut self, columns: &[&str], object: &Object) {
        for column in columns {
            self.push(object.get(*column));
        }
    }

    fn push(&mut self, value: Option<&Value>) {
        if !self.0.is_empty() {
            self.0.push(',');
        }
        match value {
            None => self.0.push_str("null"),
            Some(v) => self.write(v),
        }
    }

    fn write(&mut self, value: &Value) {
        match value {
            Value::Null => self.0.push_str("null"),
            Value::Bool(true) => self.0.push_str("true"),
            Value::Bool(false) => self.0.push_str("false"),
            Value::Number(n) => match n.as_f64() {
                Some(f) if n.is_f64() && f == 0.0 => self.0.push_str("0.0"),
                _ => {
                    let _ = write!(self.0, "{n}");
                }
            },
            Value::String(s) => self.string(s),
            Value::Array(items) => {
                self.0.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.0.push(',');
                    }
                    self.write(item);
                }
                self.0.push(']');
            }
            Value::Object(map) => {
                let mut entries: Vec<(&String, &Value)> = map.iter().collect();
                entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
                self.0.push('{');
                for (i, (k, v)) in entries.into_iter().enumerate() {
                    if i > 0 {
                        self.0.push(',');
                    }
                    self.string(k);
                    self.0.push(':');
                    self.write(v);
                }
                self.0.push('}');
            }
        }
    }

    fn string(&mut self, s: &str) {
        self.0.reserve(s.len() + 2);
        self.0.push('"');
        for c in s.chars() {
            if matches!(c, '"' | '\\') {
                self.0.push('\\');
            }
            self.0.push(c);
        }
        self.0.push('"');
    }
}

#[derive(Clone, Copy)]
struct Hit<'a> {
    count: usize,
    id: Option<&'a str>,
}

struct Partition<'a> {
    variant: Option<String>,
    columns: Option<Vec<&'a str>>,
    hits: Option<HashMap<String, Hit<'a>>>,
}

enum Signed<'a> {
    Complete,
    Missing(&'a str),
}

#[derive(Default)]
struct Partitions<'a>(Vec<Partition<'a>>);

impl<'a> Partitions<'a> {
    fn get_or_insert(&mut self, signer: Signer<'a>, variant: Option<&str>) -> &mut Partition<'a> {
        let index = match self.0.iter().position(|p| p.variant.as_deref() == variant) {
            Some(index) => index,
            None => {
                let columns = match signer.key {
                    MatchKey::Props(props) => Some(props.iter().collect()),
                    MatchKey::Value => signer.def.fields_for(variant).map(Fields::comparable_names),
                };
                self.0.push(Partition {
                    variant: variant.map(str::to_string),
                    columns,
                    hits: None,
                });
                self.0.len() - 1
            }
        };
        &mut self.0[index]
    }
}

#[derive(Clone, Copy)]
struct Signer<'a> {
    def: ObjectDef<'a>,
    key: MatchKey<'a>,
}

impl<'a> Signer<'a> {
    fn display(self) -> String {
        self.def.display_name().to_string()
    }

    fn sign(
        self,
        variant: Option<&str>,
        columns: &[&'a str],
        body: &Object,
        ids: &ClientIdRegistry,
        sig: &mut Signature,
    ) -> Result<Signed<'a>> {
        match self.key {
            MatchKey::Props(_) => {
                let fields = self.def.fields_for(variant);
                for &p in columns {
                    let Some(raw) = body.get(p) else {
                        return Ok(Signed::Missing(p));
                    };
                    let resolved = ids.resolve_match_value(self.def, fields, p, raw)?;
                    sig.push(Some(&resolved));
                }
            }
            MatchKey::Value => sig.columns(columns, body),
        }
        Ok(Signed::Complete)
    }

    fn index(
        self,
        candidates: &'a [Object],
        scope: Option<&ResolvedScope<'_>>,
        variant: Option<&str>,
        columns: &[&str],
    ) -> HashMap<String, Hit<'a>> {
        let multi = self.def.is_multi_variant();
        let mut hits: HashMap<String, Hit<'a>> = HashMap::new();
        let mut sig = Signature::default();
        for cand in candidates {
            if multi && cand.get(AT_TYPE).and_then(Value::as_str) != variant {
                continue;
            }
            if !scope.is_none_or(|s| s.contains(cand)) {
                continue;
            }
            sig.clear();
            sig.columns(columns, cand);
            let hit = hits
                .entry(mem::take(&mut sig.0))
                .or_insert(Hit { count: 0, id: None });
            hit.count += 1;
            if hit.id.is_none() {
                hit.id = object_id(cand);
            }
        }
        hits
    }
}

pub struct OpMatcher<'a> {
    signer: Signer<'a>,
    scope: Option<&'a ResolvedScope<'a>>,
    candidates: &'a [Object],
    partitions: Partitions<'a>,
    scratch: Signature,
}

impl<'a> OpMatcher<'a> {
    pub fn new(
        def: ObjectDef<'a>,
        key: MatchKey<'a>,
        scope: Option<&'a ResolvedScope<'a>>,
        candidates: &'a [Object],
    ) -> Self {
        OpMatcher {
            signer: Signer { def, key },
            scope,
            candidates,
            partitions: Partitions::default(),
            scratch: Signature::default(),
        }
    }

    fn variant<'b>(&self, at_type: Option<&'b str>) -> Option<&'b str> {
        at_type.filter(|_| self.signer.def.is_multi_variant())
    }

    pub fn find(&mut self, body: &Object, ids: &ClientIdRegistry) -> Result<Option<&'a str>> {
        let at_type = body.get(AT_TYPE).and_then(Value::as_str);
        if self.signer.def.is_multi_variant() && at_type.is_none() {
            return Err(ApplyError::MissingVariant {
                object: self.signer.display(),
            }
            .into());
        }
        let variant = self.variant(at_type);
        let (signer, scope, candidates) = (self.signer, self.scope, self.candidates);
        let Partition { columns, hits, .. } = self.partitions.get_or_insert(signer, variant);
        let Some(columns) = columns.as_deref() else {
            return Err(ApplyError::NoMatchFields {
                object: signer.display(),
            }
            .into());
        };
        if matches!(signer.key, MatchKey::Value) && columns.is_empty() {
            return Err(ApplyError::NoComparableProperties {
                object: signer.display(),
            }
            .into());
        }
        let hits = hits.get_or_insert_with(|| signer.index(candidates, scope, variant, columns));
        let sig = &mut self.scratch;
        sig.clear();
        if let Signed::Missing(property) = signer.sign(variant, columns, body, ids, sig)? {
            return Err(ApplyError::MissingMatchProperty {
                object: signer.display(),
                property: property.to_string(),
            }
            .into());
        }
        match hits.get(&sig.0).copied() {
            None => Ok(None),
            Some(Hit { count: 1, id }) => Ok(id),
            Some(Hit { count, .. }) => Err(match signer.key {
                MatchKey::Props(props) => ApplyError::AmbiguousMatch {
                    object: signer.display(),
                    count,
                    properties: props.joined(),
                },
                MatchKey::Value => ApplyError::AmbiguousValueMatch {
                    object: signer.display(),
                    count,
                },
            }
            .into()),
        }
    }

    pub fn reject_duplicates(
        &mut self,
        entries: &[(&String, Cow<'_, Object>)],
        ids: &ClientIdRegistry,
        op: OpRef,
    ) -> Result<()> {
        let signer = self.signer;
        let mut seen: HashMap<String, &str> = HashMap::with_capacity(entries.len());
        let mut sig = Signature::default();
        for (client_id, body) in entries {
            let body = body.as_ref();
            let at_type = body.get(AT_TYPE);
            let variant = self.variant(at_type.and_then(Value::as_str));
            let Some(columns) = self
                .partitions
                .get_or_insert(signer, variant)
                .columns
                .as_deref()
            else {
                continue;
            };
            sig.clear();
            sig.push(at_type);
            if !matches!(
                signer.sign(variant, columns, body, ids, &mut sig),
                Ok(Signed::Complete)
            ) {
                continue;
            }
            if let Some(first) = seen.insert(mem::take(&mut sig.0), client_id.as_str()) {
                let object = signer.display();
                let (first, second) = (first.to_string(), (*client_id).clone());
                return Err(match signer.key {
                    MatchKey::Props(props) => ApplyError::DuplicateMatchKey {
                        object,
                        op,
                        first,
                        second,
                        properties: props.joined(),
                    },
                    MatchKey::Value => ApplyError::DuplicateValues {
                        object,
                        op,
                        first,
                        second,
                    },
                }
                .into());
            }
        }
        Ok(())
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/plan/matcher/differential.rs"]
mod differential;
#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/plan/matcher/tests/mod.rs"]
mod tests;
