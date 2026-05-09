/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::error::{CliError, CliResult};
use crate::schema::{FieldType, Fields, NumberFormat};
use serde_json::{Map, Value};
use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy)]
pub enum Mode {
    Create,
    Update,
}

pub struct Sources<'a> {
    pub fields: &'a [String],
    pub json: Option<&'a str>,
    pub file: Option<&'a Path>,
    pub stdin: bool,
}

pub fn build_input(
    sources: &Sources,
    fields: &Fields,
    mode: Mode,
) -> CliResult<Map<String, Value>> {
    let mut active = 0;
    if !sources.fields.is_empty() {
        active += 1;
    }
    if sources.json.is_some() {
        active += 1;
    }
    if sources.file.is_some() {
        active += 1;
    }
    if sources.stdin {
        active += 1;
    }
    if active == 0 {
        let payloadless_create = matches!(mode, Mode::Create) && fields.properties.is_empty();
        if !payloadless_create {
            return Err(CliError::msg(
                "no input provided; use --field, --json, --file, or --stdin",
            ));
        }
    }
    if active > 1 {
        return Err(CliError::msg(
            "input sources are mutually exclusive: pick one of --field, --json, --file, --stdin",
        ));
    }

    if let Some(json) = sources.json {
        return parse_json_object(json);
    }
    if let Some(path) = sources.file {
        let bytes = std::fs::read(path)?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| CliError::msg(format!("{}: invalid UTF-8: {e}", path.display())))?;
        return parse_json_object(text);
    }
    if sources.stdin {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        return parse_json_object(&buf);
    }

    fields_to_map(sources.fields, fields, mode)
}

fn parse_json_object(text: &str) -> CliResult<Map<String, Value>> {
    let v: Value = serde_json::from_str(text.trim())
        .map_err(|e| CliError::msg(format!("invalid JSON: {e}")))?;
    match v {
        Value::Object(m) => Ok(m),
        _ => Err(CliError::msg("input must be a JSON object")),
    }
}

fn fields_to_map(entries: &[String], fields: &Fields, mode: Mode) -> CliResult<Map<String, Value>> {
    let mut out = Map::new();
    for raw in entries {
        let (key, value_raw) = split_kv(raw)?;
        let (key_canon, field_type) = resolve_key(key, fields, mode)?;

        if matches!(mode, Mode::Create) && key_canon.contains('/') {
            return Err(CliError::msg(format!(
                "`--field` keys for `create` must be top-level properties (got `{key_canon}`)"
            )));
        }

        if out.contains_key(&key_canon) {
            return Err(CliError::msg(format!(
                "duplicate --field key `{key_canon}`"
            )));
        }

        let coerced = coerce_field_value(value_raw, field_type.as_ref(), mode)?;
        out.insert(key_canon, coerced);
    }
    Ok(out)
}

fn split_kv(raw: &str) -> CliResult<(&str, &str)> {
    if let Some(first) = raw.chars().next()
        && (first == '"' || first == '\'')
    {
        let rest = &raw[first.len_utf8()..];
        let Some(close) = rest.find(first) else {
            return Err(CliError::msg(format!(
                "invalid --field `{raw}`: unterminated quoted key"
            )));
        };
        let key = &rest[..close];
        if key.is_empty() {
            return Err(CliError::msg(format!(
                "invalid --field `{raw}`: empty quoted key"
            )));
        }
        let after = &rest[close + first.len_utf8()..];
        let Some(value) = after.strip_prefix('=') else {
            return Err(CliError::msg(format!(
                "invalid --field `{raw}`: expected `=` after quoted key"
            )));
        };
        return Ok((key, value));
    }
    match raw.split_once('=') {
        Some((k, v)) if !k.is_empty() => Ok((k, v)),
        _ => Err(CliError::msg(format!(
            "invalid --field `{raw}`: expected KEY=VALUE"
        ))),
    }
}

fn resolve_key(key: &str, fields: &Fields, mode: Mode) -> CliResult<(String, Option<FieldType>)> {
    if key.eq_ignore_ascii_case("@type") {
        return Ok(("@type".to_string(), None));
    }

    match mode {
        Mode::Create => {
            let matched = fields
                .properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(k, f)| (k.clone(), f.typ.clone()));
            match matched {
                Some((canon, typ)) => Ok((canon, Some(typ))),
                None => Err(CliError::UnknownField(key.to_string())),
            }
        }
        Mode::Update => {
            let (head, rest) = match key.split_once('/') {
                Some((h, r)) => (h, Some(r)),
                None => (key, None),
            };
            let matched = fields
                .properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(head));
            let (head_canon, typ) = match matched {
                Some((k, f)) => (k.clone(), Some(f.typ.clone())),
                None => return Err(CliError::UnknownField(head.to_string())),
            };
            let canon = match rest {
                None => head_canon,
                Some(r) => {
                    let mut s = head_canon;
                    s.push('/');
                    s.push_str(r);
                    s
                }
            };

            let typ = if rest.is_some() { None } else { typ };
            Ok((canon, typ))
        }
    }
}

pub fn coerce_field_value(
    raw: &str,
    field_type: Option<&FieldType>,
    mode: Mode,
) -> CliResult<Value> {
    let trimmed = raw.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return serde_json::from_str::<Value>(raw)
            .map_err(|e| CliError::msg(format!("invalid JSON in --field value: {e}")));
    }

    if raw == "null" {
        return match (field_type, mode) {
            (_, Mode::Update) => Ok(Value::Null),
            (Some(ft), Mode::Create) if type_is_nullable(ft) => Ok(Value::Null),
            (None, Mode::Create) => Ok(Value::Null),
            (Some(_), Mode::Create) => Err(CliError::msg(
                "null is not allowed for non-nullable fields on create",
            )),
        };
    }

    if let Some(ft) = field_type {
        match ft {
            FieldType::Boolean => match raw.to_ascii_lowercase().as_str() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(CliError::msg(format!("expected boolean, got `{raw}`"))),
            },
            FieldType::Number {
                format: NumberFormat::Float,
                ..
            } => raw
                .parse::<f64>()
                .map_err(|_| CliError::msg(format!("expected number, got `{raw}`")))
                .and_then(|n| {
                    serde_json::Number::from_f64(n)
                        .map(Value::Number)
                        .ok_or_else(|| CliError::msg(format!("non-finite number `{raw}`")))
                }),
            FieldType::Number { .. } => raw
                .parse::<i64>()
                .map(|n| Value::Number(n.into()))
                .map_err(|_| CliError::msg(format!("expected integer, got `{raw}`"))),
            FieldType::Object { .. }
            | FieldType::ObjectList { .. }
            | FieldType::Set { .. }
            | FieldType::Map { .. } => Err(CliError::msg(
                "field requires a JSON object/array value (wrap in `{...}` or `[...]`)",
            )),
            _ => Ok(Value::String(raw.to_string())),
        }
    } else {
        if raw.eq_ignore_ascii_case("true") {
            return Ok(Value::Bool(true));
        }
        if raw.eq_ignore_ascii_case("false") {
            return Ok(Value::Bool(false));
        }
        if let Ok(n) = raw.parse::<i64>() {
            return Ok(Value::Number(n.into()));
        }
        if let Ok(n) = raw.parse::<f64>()
            && let Some(num) = serde_json::Number::from_f64(n)
        {
            return Ok(Value::Number(num));
        }
        Ok(Value::String(raw.to_string()))
    }
}

fn type_is_nullable(ft: &FieldType) -> bool {
    match ft {
        FieldType::String { nullable, .. }
        | FieldType::Number { nullable, .. }
        | FieldType::UtcDateTime { nullable }
        | FieldType::Enum { nullable, .. }
        | FieldType::ObjectId { nullable, .. }
        | FieldType::Object { nullable, .. } => *nullable,
        _ => false,
    }
}

pub fn parse_ids_from_stdin() -> CliResult<Vec<String>> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    parse_ids(&buf)
}

pub fn parse_ids(raw: &str) -> CliResult<Vec<String>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(trimmed) {
        let mut out = Vec::with_capacity(arr.len());
        for v in arr {
            match v {
                Value::String(s) => out.push(s),
                _ => {
                    return Err(CliError::msg("ids array must contain only strings"));
                }
            }
        }
        return Ok(out);
    }
    Ok(trimmed
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_fields() -> Fields {
        Fields::default()
    }

    fn fields_with(name: &str) -> Fields {
        use crate::schema::{Field, FieldType, FieldUpdate, StringFormat};
        let mut f = Fields::default();
        f.properties.insert(
            name.to_string(),
            Field {
                description: String::new(),
                typ: FieldType::String {
                    format: StringFormat::String,
                    min_length: None,
                    max_length: None,
                    nullable: false,
                },
                update: FieldUpdate::Mutable,
                enterprise: false,
            },
        );
        f
    }

    #[test]
    fn split_kv_basic() {
        assert_eq!(split_kv("name=foo").unwrap(), ("name", "foo"));
        assert_eq!(split_kv("a=b=c").unwrap(), ("a", "b=c"));
    }

    #[test]
    fn split_kv_rejects_no_eq() {
        assert!(split_kv("foo").is_err());
        assert!(split_kv("=bar").is_err());
    }

    #[test]
    fn split_kv_quoted() {
        assert_eq!(
            split_kv(r#""path/with=eq"=value"#).unwrap(),
            ("path/with=eq", "value")
        );
        assert_eq!(split_kv("'a b c'=hello").unwrap(), ("a b c", "hello"));
        assert_eq!(split_kv(r#""x"="#).unwrap(), ("x", ""));
    }

    #[test]
    fn split_kv_quoted_errors() {
        assert!(split_kv(r#""unterminated"#).is_err());
        assert!(split_kv(r#""a"b"#).is_err());
        assert!(split_kv(r#""""#).is_err());
    }

    #[test]
    fn coerce_lax_numbers_and_bools() {
        assert_eq!(
            coerce_field_value("42", None, Mode::Update).unwrap(),
            Value::Number(42.into())
        );
        assert_eq!(
            coerce_field_value("true", None, Mode::Update).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            coerce_field_value("hello", None, Mode::Update).unwrap(),
            Value::String("hello".into())
        );
        assert_eq!(
            coerce_field_value("null", None, Mode::Update).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn coerce_json_object() {
        let v = coerce_field_value("{\"a\":1}", None, Mode::Create).unwrap();
        assert_eq!(v, serde_json::json!({"a": 1}));
    }

    #[test]
    fn parse_ids_handles_json_and_csv() {
        assert_eq!(parse_ids(r#"["a","b","c"]"#).unwrap(), vec!["a", "b", "c"]);
        assert_eq!(parse_ids("a, b, c").unwrap(), vec!["a", "b", "c"]);
        assert_eq!(parse_ids("a b c").unwrap(), vec!["a", "b", "c"]);
        assert_eq!(parse_ids("a\nb\nc").unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn build_input_rejects_multiple_sources() {
        let s = Sources {
            fields: &["a=b".to_string()],
            json: Some("{}"),
            file: None,
            stdin: false,
        };
        assert!(build_input(&s, &empty_fields(), Mode::Create).is_err());
    }

    #[test]
    fn build_input_requires_at_least_one_source_when_schema_has_fields() {
        let s = Sources {
            fields: &[],
            json: None,
            file: None,
            stdin: false,
        };
        let err = build_input(&s, &fields_with("name"), Mode::Create).unwrap_err();
        assert!(format!("{err}").contains("no input provided"));
    }

    #[test]
    fn build_input_allows_no_sources_for_payloadless_create() {
        let s = Sources {
            fields: &[],
            json: None,
            file: None,
            stdin: false,
        };
        let out = build_input(&s, &empty_fields(), Mode::Create).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn build_input_still_requires_source_for_update_with_no_fields() {
        let s = Sources {
            fields: &[],
            json: None,
            file: None,
            stdin: false,
        };
        let err = build_input(&s, &empty_fields(), Mode::Update).unwrap_err();
        assert!(format!("{err}").contains("no input provided"));
    }
}
