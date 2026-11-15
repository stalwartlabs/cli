/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use super::{CompareOp, FILTER};
use crate::error::Result;
use crate::input::InputError;
use crate::schema::Filter;
use crate::schema::view::ViewDef;
use crate::view::time::UtcTimestamp;
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug)]
pub enum FilterValue<'a> {
    One(&'a str),
    Many(&'a [Box<str>]),
}

#[derive(Clone, Copy, Debug)]
pub struct AppliedFilter<'a> {
    pub filter: &'a Filter,
    pub op: CompareOp,
    pub value: FilterValue<'a>,
}

pub fn build_list_filter(view: &ViewDef<'_>, applied: &[AppliedFilter<'_>]) -> Result<Value> {
    let fields = view.object.all_fields();
    let stalwart = view.object.name().starts_with("x:");
    let statics = view.static_filter.map_or(0, |s| s.len());
    let mut filter = Map::with_capacity(applied.len() + statics);
    for entry in applied {
        let name = entry.filter.field();
        let comparable = matches!(entry.filter, Filter::Integer { .. } | Filter::Date { .. });
        if entry.op != CompareOp::Eq && !comparable {
            return Err(InputError::ComparisonNotAllowed(entry.op.symbol()).into());
        }
        let raw = match entry.value {
            FilterValue::Many([]) => continue,
            FilterValue::Many(ids) => {
                let ids = ids.iter().map(|id| Value::String(id.as_ref().to_owned()));
                filter.insert(name.to_owned(), Value::Array(ids.collect()));
                continue;
            }
            FilterValue::One(raw) => raw.trim(),
        };
        if raw.is_empty() {
            continue;
        }
        match entry.filter {
            Filter::Date { .. } => insert_date(&mut filter, name, raw, entry.op, stalwart),
            Filter::Integer { .. } => {
                let ty = fields.get(name).map(|field| &field.typ);
                filter.insert(keyed(name, entry.op), FILTER.filter_value(raw, ty)?);
            }
            _ => {
                filter.insert(name.to_owned(), Value::String(raw.to_owned()));
            }
        }
    }
    if let Some(statics) = view.static_filter {
        filter.extend(statics.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    Ok(Value::Object(filter))
}

fn keyed(name: &str, op: CompareOp) -> String {
    let suffix = op.suffix();
    let mut key = String::with_capacity(name.len() + suffix.len());
    key.push_str(name);
    key.push_str(suffix);
    key
}

fn iso(at: UtcTimestamp) -> Value {
    let mut out = String::with_capacity(20);
    let _ = at.format_iso(&mut out);
    Value::String(out)
}

fn insert_date(
    filter: &mut Map<String, Value>,
    name: &str,
    raw: &str,
    op: CompareOp,
    stalwart: bool,
) {
    let Some(day) = UtcTimestamp::parse_date(raw) else {
        let op = if stalwart { op } else { CompareOp::Eq };
        filter.insert(keyed(name, op), Value::String(raw.to_owned()));
        return;
    };
    match (stalwart, op) {
        (false, _) => {
            filter.insert(name.to_owned(), iso(day));
        }
        (true, CompareOp::Eq) => {
            filter.insert(keyed(name, CompareOp::Gte), iso(day));
            filter.insert(keyed(name, CompareOp::Lt), iso(day.next_day_start()));
        }
        (true, op) => {
            filter.insert(keyed(name, op), iso(day));
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/input/filter/list/tests.rs"]
mod tests;
