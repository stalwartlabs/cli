/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::input::{InputError, Mode};
use crate::schema::{FieldType, NumberFormat};
use serde_json::{Number, Value};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoolWords {
    Strict,
    Lenient,
}

#[derive(Clone, Copy, Debug)]
pub struct Coercer {
    bools: BoolWords,
}

impl BoolWords {
    fn parse(self, raw: &str) -> Option<bool> {
        match self {
            BoolWords::Strict => hashify::map_ignore_case!(
                raw.as_bytes(),
                bool,
                "true" => true,
                "false" => false,
            ),
            BoolWords::Lenient => hashify::map_ignore_case!(
                raw.as_bytes(),
                bool,
                "true" => true,
                "yes" => true,
                "1" => true,
                "false" => false,
                "no" => false,
                "0" => false,
            ),
        }
        .copied()
    }
}

impl Coercer {
    pub const fn new(bools: BoolWords) -> Self {
        Coercer { bools }
    }

    pub fn coerce(&self, raw: &str, ty: Option<&FieldType>, mode: Mode) -> Result<Value> {
        if raw.trim_start().starts_with(['{', '[']) {
            return serde_json::from_str(raw)
                .map_err(|err| InputError::InvalidFieldJson(err).into());
        }
        if raw == "null" {
            return match (ty, mode) {
                (Some(ty), Mode::Create) if !ty.is_nullable() => {
                    Err(InputError::NullNotAllowed.into())
                }
                _ => Ok(Value::Null),
            };
        }
        Ok(match ty {
            Some(FieldType::Boolean) => self.boolean(raw)?,
            Some(FieldType::Number { format, .. }) => Self::number(raw, format)?,
            Some(ty) if ty.is_composite() => return Err(InputError::CompositeNeedsJson.into()),
            Some(_) => Value::String(raw.to_owned()),
            None => self.guess(raw),
        })
    }

    pub fn filter_value(&self, raw: &str, ty: Option<&FieldType>) -> Result<Value> {
        Ok(match ty {
            Some(FieldType::Boolean) => self.boolean(raw)?,
            Some(FieldType::Number { format, .. }) => Self::number(raw, format)?,
            _ => Value::String(raw.to_owned()),
        })
    }

    fn boolean(&self, raw: &str) -> Result<Value, InputError> {
        self.bools
            .parse(raw)
            .map(Value::Bool)
            .ok_or_else(|| InputError::ExpectedBoolean(raw.to_owned()))
    }

    fn guess(&self, raw: &str) -> Value {
        if let Some(flag) = self.bools.parse(raw) {
            return Value::Bool(flag);
        }
        if let Ok(n) = raw.parse::<i64>() {
            return Value::from(n);
        }
        match raw.parse::<f64>().ok().and_then(Number::from_f64) {
            Some(n) => Value::Number(n),
            None => Value::String(raw.to_owned()),
        }
    }

    fn number(raw: &str, format: &NumberFormat) -> Result<Value, InputError> {
        match format {
            NumberFormat::Float => {
                let n: f64 = raw
                    .parse()
                    .map_err(|_| InputError::ExpectedNumber(raw.to_owned()))?;
                Number::from_f64(n)
                    .map(Value::Number)
                    .ok_or_else(|| InputError::NonFinite(raw.to_owned()))
            }
            _ => raw
                .parse::<i64>()
                .map(Value::from)
                .map_err(|_| InputError::ExpectedInteger(raw.to_owned())),
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/input/coerce/tests.rs"]
mod tests;
