/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::input::{BoolWords, Coercer, InputError};
use crate::schema::FieldType;
use crate::schema::object::FieldSet;
use serde_json::{Map, Value};

pub mod list;

const FILTER: Coercer = Coercer::new(BoolWords::Lenient);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompareOp {
    Eq,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Where<'a> {
    pub field: &'a str,
    pub op: CompareOp,
    pub value: &'a str,
}

impl CompareOp {
    const LONGEST_FIRST: [CompareOp; 5] = [
        CompareOp::Gte,
        CompareOp::Lte,
        CompareOp::Gt,
        CompareOp::Lt,
        CompareOp::Eq,
    ];

    pub fn suffix(self) -> &'static str {
        match self {
            CompareOp::Eq => "",
            CompareOp::Gt => "IsGreaterThan",
            CompareOp::Gte => "IsGreaterThanOrEqual",
            CompareOp::Lt => "IsLessThan",
            CompareOp::Lte => "IsLessThanOrEqual",
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            CompareOp::Eq => "=",
            CompareOp::Gt => ">",
            CompareOp::Gte => ">=",
            CompareOp::Lt => "<",
            CompareOp::Lte => "<=",
        }
    }
}

impl<'a> Where<'a> {
    pub fn parse(raw: &'a str) -> Result<Self, InputError> {
        let invalid = || InputError::InvalidWhere(raw.to_owned());
        let (field, _) = raw.split_once(['<', '>', '=']).ok_or_else(invalid)?;
        if field.is_empty() {
            return Err(invalid());
        }
        let rest = raw.strip_prefix(field).ok_or_else(invalid)?;
        let (op, value) = CompareOp::LONGEST_FIRST
            .into_iter()
            .find_map(|op| rest.strip_prefix(op.symbol()).map(|value| (op, value)))
            .ok_or_else(invalid)?;
        Ok(Where {
            field: field.trim(),
            op,
            value,
        })
    }
}

pub fn build_filter(fields: FieldSet<'_>, clauses: &[Where<'_>]) -> Result<Value> {
    let mut filter = Map::with_capacity(clauses.len());
    for clause in clauses {
        let matched = fields.get_ci(clause.field);
        if clause.op != CompareOp::Eq
            && let Some((_, field)) = matched
            && !matches!(
                field.typ,
                FieldType::Number { .. } | FieldType::UtcDateTime { .. }
            )
        {
            return Err(InputError::ComparisonNotAllowed(clause.op.symbol()).into());
        }
        let (name, ty) = match matched {
            Some((name, field)) => (name, Some(&field.typ)),
            None => (clause.field, None),
        };
        let suffix = clause.op.suffix();
        let mut key = String::with_capacity(name.len() + suffix.len());
        key.push_str(name);
        key.push_str(suffix);
        filter.insert(key, FILTER.filter_value(clause.value, ty)?);
    }
    Ok(Value::Object(filter))
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/input/filter/tests.rs"]
mod tests;
