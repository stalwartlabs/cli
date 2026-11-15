/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::{FieldType, Fields, Schema};
use serde_json::{Map, Value};
use std::fmt::Write;
use std::iter;

const MATCH: &str = "match";
pub const ELSE: &str = "else";
pub const IF: &str = "if";
pub const THEN: &str = "then";
const PARSE_PREFIX: &str = "Error parsing '";
const PARSE_ELSE: &str = " expression: ";
const PARSE_CONDITION: &str = " expression in condition #";
const MISSING_ELSE: &str = "Missing 'else' block";
const QUOTE: char = '"';
pub const KEYWORD_WIDTH: usize = 5;
pub const EMPTY: &str = "The expression is empty";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expression<S> {
    pub rules: Vec<Rule<S>>,
    pub default: S,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rule<S> {
    pub condition: S,
    pub result: S,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Condition(usize),
    Result(usize),
    Default,
}

#[derive(Clone, Copy)]
enum Side {
    If,
    Then,
    Else,
}

impl Schema {
    pub fn is_expression(&self, object_name: &str) -> bool {
        let Some(fields) = self.single_fields(object_name) else {
            return false;
        };
        fields.properties.len() == 2
            && is_text(fields, ELSE)
            && matches!(
                fields.properties.get(MATCH).map(|field| &field.typ),
                Some(FieldType::ObjectList { object_name, .. })
                    if self.single_fields(object_name).is_some_and(|rule| {
                        rule.properties.len() == 2 && is_text(rule, IF) && is_text(rule, THEN)
                    })
            )
    }

    fn single_fields(&self, object_name: &str) -> Option<&Fields> {
        self.fields
            .get(self.schemas.get(object_name)?.single_name()?)
    }
}

fn is_text(fields: &Fields, name: &str) -> bool {
    matches!(
        fields.properties.get(name).map(|field| &field.typ),
        Some(FieldType::String { .. })
    )
}

fn text<'a>(map: &'a Map<String, Value>, name: &str) -> &'a str {
    map.get(name).and_then(Value::as_str).unwrap_or_default()
}

impl<'a> Expression<&'a str> {
    pub fn parse(value: &'a Value) -> Option<Self> {
        let map = value.as_object()?;
        let rules = match map.get(MATCH) {
            Some(Value::Object(items)) => {
                let mut keyed: Vec<(usize, Rule<&str>)> = items
                    .iter()
                    .map(|(key, item)| (key.parse().unwrap_or(usize::MAX), Rule::parse(item)))
                    .collect();
                keyed.sort_unstable_by_key(|(index, _)| *index);
                keyed.into_iter().map(|(_, rule)| rule).collect()
            }
            Some(Value::Array(items)) => items.iter().map(Rule::parse).collect(),
            _ => Vec::new(),
        };
        Some(Self {
            rules,
            default: text(map, ELSE),
        })
    }

    pub fn owned(&self) -> Expression<String> {
        Expression {
            rules: self
                .rules
                .iter()
                .map(|rule| Rule {
                    condition: rule.condition.to_owned(),
                    result: rule.result.to_owned(),
                })
                .collect(),
            default: self.default.to_owned(),
        }
    }
}

impl<'a> Rule<&'a str> {
    fn parse(value: &'a Value) -> Self {
        match value.as_object() {
            Some(map) => Rule {
                condition: text(map, IF),
                result: text(map, THEN),
            },
            None => Rule::default(),
        }
    }
}

impl Expression<String> {
    pub fn into_value(self) -> Value {
        let mut map = Map::with_capacity(2);
        map.insert(
            MATCH.into(),
            Value::Object(
                self.rules
                    .into_iter()
                    .enumerate()
                    .map(|(index, rule)| {
                        let mut item = Map::with_capacity(2);
                        item.insert(IF.into(), Value::String(rule.condition));
                        item.insert(THEN.into(), Value::String(rule.result));
                        (index.to_string(), Value::Object(item))
                    })
                    .collect(),
            ),
        );
        map.insert(ELSE.into(), Value::String(self.default));
        Value::Object(map)
    }

    pub fn text(&self, part: Part) -> Option<&str> {
        match part {
            Part::Condition(index) => self.rules.get(index).map(|rule| &*rule.condition),
            Part::Result(index) => self.rules.get(index).map(|rule| &*rule.result),
            Part::Default => Some(&self.default),
        }
    }

    pub fn text_mut(&mut self, part: Part) -> Option<&mut String> {
        match part {
            Part::Condition(index) => self.rules.get_mut(index).map(|rule| &mut rule.condition),
            Part::Result(index) => self.rules.get_mut(index).map(|rule| &mut rule.result),
            Part::Default => Some(&mut self.default),
        }
    }
}

impl<S> Expression<S> {
    pub fn parts(&self) -> impl Iterator<Item = (Part, &S)> {
        self.rules
            .iter()
            .enumerate()
            .flat_map(|(index, rule)| {
                [
                    (Part::Condition(index), &rule.condition),
                    (Part::Result(index), &rule.result),
                ]
            })
            .chain(iter::once((Part::Default, &self.default)))
    }
}

impl<S: AsRef<str>> Expression<S> {
    pub fn empty_part(&self) -> Option<Part> {
        if self.default.as_ref().is_empty() && !self.rules.is_empty() {
            return Some(Part::Default);
        }
        self.rules.iter().enumerate().find_map(|(index, rule)| {
            if rule.condition.as_ref().is_empty() {
                Some(Part::Condition(index))
            } else if rule.result.as_ref().is_empty() {
                Some(Part::Result(index))
            } else {
                None
            }
        })
    }
}

impl Part {
    pub fn keyword(self) -> &'static str {
        match self {
            Part::Condition(_) => IF,
            Part::Result(_) => THEN,
            Part::Default => ELSE,
        }
    }

    pub fn rule(self) -> Option<usize> {
        match self {
            Part::Condition(index) | Part::Result(index) => Some(index),
            Part::Default => None,
        }
    }

    pub fn describe(self, message: &str, out: &mut String) {
        let _ = match self {
            Part::Condition(index) => write!(
                out,
                "{PARSE_PREFIX}{IF}'{PARSE_CONDITION}{}: {message}",
                index + 1
            ),
            Part::Result(index) => write!(
                out,
                "{PARSE_PREFIX}{THEN}'{PARSE_CONDITION}{}: {message}",
                index + 1
            ),
            Part::Default => write!(out, "{PARSE_PREFIX}{ELSE}'{PARSE_ELSE}{message}"),
        };
    }

    pub fn locate<'t>(text: &'t str) -> Option<(Part, &'t str)> {
        if let Some((at, _)) = text.match_indices(MISSING_ELSE).next() {
            let message = text.get(at..)?;
            let message = match text.get(..at).is_some_and(|before| before.ends_with(QUOTE)) {
                true => message.strip_suffix(QUOTE).unwrap_or(message),
                false => message,
            };
            return Some((Part::Default, message));
        }
        let (before, rest) = text.split_once(PARSE_PREFIX)?;
        let rest = match before.ends_with(QUOTE) {
            true => rest.strip_suffix(QUOTE).unwrap_or(rest),
            false => rest,
        };
        let (name, rest) = rest.split_once('\'')?;
        let side = hashify::map!(
            name.as_bytes(),
            Side,
            "if" => Side::If,
            "then" => Side::Then,
            "else" => Side::Else,
        )?;
        let rule = |rest: &'t str| {
            let (number, message) = rest.strip_prefix(PARSE_CONDITION)?.split_once(": ")?;
            Some((number.parse::<usize>().ok()?.checked_sub(1)?, message))
        };
        match side {
            Side::If => rule(rest).map(|(index, message)| (Part::Condition(index), message)),
            Side::Then => rule(rest).map(|(index, message)| (Part::Result(index), message)),
            Side::Else => Some((Part::Default, rest.strip_prefix(PARSE_ELSE)?)),
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/schema/expression/tests.rs"]
mod tests;
