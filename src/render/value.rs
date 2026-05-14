/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::render::Ansi;
use crate::schema::{FieldType, MapValueType, NumberFormat, ObjectSchema, ScalarType, Schema};
use crate::util::display_cache::DisplayCache;
use serde_json::Value;

pub fn render_inline(
    out: &mut String,
    schema: &Schema,
    value: &Value,
    field_type: &FieldType,
    cache: &DisplayCache,
    ansi: Ansi,
) {
    if value.is_null() {
        push_none(out, ansi);
        return;
    }
    match field_type {
        FieldType::String { .. } => match value.as_str() {
            Some(s) => out.push_str(s),
            None => push_raw_json(out, value),
        },
        FieldType::Number { format, .. } => render_number(out, value, format),
        FieldType::UtcDateTime { .. } => match value.as_str() {
            Some(s) => out.push_str(s),
            None => push_raw_json(out, value),
        },
        FieldType::Boolean => match value.as_bool() {
            Some(true) => out.push_str("Yes"),
            Some(false) => out.push_str("No"),
            None => push_raw_json(out, value),
        },
        FieldType::Enum { enum_name, .. } => render_enum(out, schema, enum_name, value, ansi),
        FieldType::BlobId => {
            out.push_str("<blob: ");
            if let Some(s) = value.as_str() {
                out.push_str(s);
            }
            out.push('>');
        }
        FieldType::ObjectId { object_name, .. } => render_object_id(out, cache, object_name, value),
        FieldType::Set { class, .. } => render_set(out, schema, value, class, cache, ansi),
        FieldType::Map {
            key_class,
            value_class,
            ..
        } => render_map_inline(out, schema, value, key_class, value_class, cache, ansi),
        FieldType::Object { object_name, .. } => match variant_label(schema, object_name, value) {
            Some(label) => out.push_str(label),
            None => out.push_str("<object>"),
        },
        FieldType::ObjectList { .. } => {
            let count = value.as_object().map(|o| o.len()).unwrap_or(0);
            out.push_str("<list:");
            out.push_str(&count.to_string());
            out.push('>');
        }
    }
}

fn variant_label<'a>(schema: &'a Schema, object_name: &str, value: &Value) -> Option<&'a str> {
    let ObjectSchema::Multiple { variants } = schema.schemas.get(object_name)? else {
        return None;
    };
    let at_type = value.get("@type")?.as_str()?;
    variants
        .iter()
        .find(|v| v.name == at_type)
        .map(|v| v.label.as_str())
}

fn push_none(out: &mut String, ansi: Ansi) {
    out.push_str(ansi.dim());
    out.push_str("<none>");
    out.push_str(ansi.reset());
}

fn push_raw_json(out: &mut String, value: &Value) {
    match serde_json::to_string(value) {
        Ok(s) => out.push_str(&s),
        Err(_) => out.push_str("<unprintable>"),
    }
}

fn render_number(out: &mut String, value: &Value, format: &NumberFormat) {
    let Some(n) = value.as_f64() else {
        push_raw_json(out, value);
        return;
    };
    match format {
        NumberFormat::Size => render_size(out, n),
        NumberFormat::Duration => render_duration(out, n),
        _ => match value.as_i64() {
            Some(i) => out.push_str(&i.to_string()),
            None => out.push_str(&n.to_string()),
        },
    }
}

fn render_size(out: &mut String, bytes: f64) {
    if bytes <= 0.0 {
        out.push_str("0 B");
        return;
    }
    const UNITS: &[(&str, f64)] = &[
        ("TB", (1024u64 * 1024 * 1024 * 1024) as f64),
        ("GB", (1024u64 * 1024 * 1024) as f64),
        ("MB", (1024u64 * 1024) as f64),
        ("KB", 1024.0),
    ];
    for (label, unit) in UNITS {
        if bytes >= *unit {
            let v = bytes / *unit;
            if v < 100.0 {
                out.push_str(&format!("{:.1} {}", v, label));
            } else {
                out.push_str(&format!("{:.0} {}", v, label));
            }
            return;
        }
    }
    out.push_str(&format!("{:.0} B", bytes));
}

const DURATION_UNITS: &[(u64, &str)] = &[
    (86_400_000, "d"),
    (3_600_000, "h"),
    (60_000, "m"),
    (1_000, "s"),
    (1, "ms"),
];

fn render_duration(out: &mut String, ms: f64) {
    if ms == 0.0 {
        out.push_str("0 ms");
        return;
    }
    let negative = ms.is_sign_negative();
    let ms_abs = ms.abs() as u64;
    if ms_abs == 0 {
        out.push_str("0 ms");
        return;
    }
    for i in 0..DURATION_UNITS.len() {
        let (unit, label) = DURATION_UNITS[i];
        if ms_abs >= unit {
            if negative {
                out.push('-');
            }
            let whole = ms_abs / unit;
            let rem = ms_abs % unit;
            out.push_str(&whole.to_string());
            out.push(' ');
            out.push_str(label);
            if i + 1 < DURATION_UNITS.len() && rem > 0 {
                let (next_unit, next_label) = DURATION_UNITS[i + 1];
                let next_whole = rem / next_unit;
                if next_whole > 0 {
                    out.push(' ');
                    out.push_str(&next_whole.to_string());
                    out.push(' ');
                    out.push_str(next_label);
                }
            }
            return;
        }
    }
    out.push_str("0 ms");
}

fn render_enum(out: &mut String, schema: &Schema, enum_name: &str, value: &Value, ansi: Ansi) {
    let Some(variant_name) = value.as_str() else {
        push_raw_json(out, value);
        return;
    };
    let Some(variants) = schema.enums.get(enum_name) else {
        out.push_str(variant_name);
        return;
    };
    let Some(variant) = variants.iter().find(|v| v.name == variant_name) else {
        out.push_str(variant_name);
        return;
    };
    match &variant.color {
        Some(c) => out.push_str(&ansi.named(c)),
        None => out.push_str(ansi.blue()),
    }
    out.push_str(&variant.label);
    out.push_str(ansi.reset());
}

fn render_object_id(out: &mut String, cache: &DisplayCache, object_name: &str, value: &Value) {
    let Some(id) = value.as_str() else {
        push_raw_json(out, value);
        return;
    };
    match cache.get(object_name, id) {
        Some(label) => {
            out.push_str(label);
            out.push_str(" (id: ");
            out.push_str(id);
            out.push(')');
        }
        None => out.push_str(id),
    }
}

fn render_set(
    out: &mut String,
    schema: &Schema,
    value: &Value,
    class: &ScalarType,
    cache: &DisplayCache,
    ansi: Ansi,
) {
    let Some(map) = value.as_object() else {
        push_raw_json(out, value);
        return;
    };
    if map.is_empty() {
        push_none(out, ansi);
        return;
    }
    let mut first = true;
    for key in map.keys() {
        if !first {
            out.push_str(", ");
        }
        first = false;
        match class {
            ScalarType::String { .. } => out.push_str(key),
            ScalarType::ObjectId { object_name } => match cache.get(object_name, key) {
                Some(label) => {
                    out.push_str(label);
                    out.push_str(" (id: ");
                    out.push_str(key);
                    out.push(')');
                }
                None => out.push_str(key),
            },
            ScalarType::Enum { enum_name } => {
                let variants = schema.enums.get(enum_name);
                let label = variants.and_then(|vs| vs.iter().find(|v| v.name == *key));
                match label {
                    Some(v) => {
                        match &v.color {
                            Some(c) => out.push_str(&ansi.named(c)),
                            None => out.push_str(ansi.blue()),
                        }
                        out.push_str(&v.label);
                        out.push_str(ansi.reset());
                    }
                    None => out.push_str(key),
                }
            }
        }
    }
}

fn render_map_inline(
    out: &mut String,
    schema: &Schema,
    value: &Value,
    key_class: &ScalarType,
    value_class: &MapValueType,
    cache: &DisplayCache,
    ansi: Ansi,
) {
    if matches!(value_class, MapValueType::Object { .. }) {
        let count = value.as_object().map(|o| o.len()).unwrap_or(0);
        out.push_str("<map:");
        out.push_str(&count.to_string());
        out.push('>');
        return;
    }
    let Some(map) = value.as_object() else {
        push_raw_json(out, value);
        return;
    };
    if map.is_empty() {
        push_none(out, ansi);
        return;
    }

    let mut first = true;
    for (k, v) in map {
        if !first {
            out.push_str(", ");
        }
        first = false;
        render_scalar_key(out, schema, key_class, k, cache, ansi);
        out.push_str(" → ");
        render_map_value(out, schema, value_class, v, ansi);
    }
}

fn render_scalar_key(
    out: &mut String,
    schema: &Schema,
    class: &ScalarType,
    key: &str,
    cache: &DisplayCache,
    ansi: Ansi,
) {
    match class {
        ScalarType::String { .. } => out.push_str(key),
        ScalarType::ObjectId { object_name } => match cache.get(object_name, key) {
            Some(label) => {
                out.push_str(label);
                out.push_str(" (id: ");
                out.push_str(key);
                out.push(')');
            }
            None => out.push_str(key),
        },
        ScalarType::Enum { enum_name } => {
            let variants = schema.enums.get(enum_name);
            match variants.and_then(|vs| vs.iter().find(|v| v.name == *key)) {
                Some(v) => {
                    match &v.color {
                        Some(c) => out.push_str(&ansi.named(c)),
                        None => out.push_str(ansi.blue()),
                    }
                    out.push_str(&v.label);
                    out.push_str(ansi.reset());
                }
                None => out.push_str(key),
            }
        }
    }
}

fn render_map_value(
    out: &mut String,
    schema: &Schema,
    class: &MapValueType,
    value: &Value,
    ansi: Ansi,
) {
    if value.is_null() {
        push_none(out, ansi);
        return;
    }
    match class {
        MapValueType::String { format, .. } => {
            let stub = FieldType::String {
                format: format.clone(),
                min_length: None,
                max_length: None,
                nullable: false,
            };
            render_inline(out, schema, value, &stub, &DisplayCache::new(), ansi);
        }
        MapValueType::Number { format, .. } => render_number(out, value, format),
        MapValueType::Enum { enum_name } => render_enum(out, schema, enum_name, value, ansi),
        MapValueType::Object { .. } => out.push_str("<object>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::StringFormat;
    use serde_json::json;

    fn s() -> String {
        String::new()
    }

    fn string_type(format: StringFormat) -> FieldType {
        FieldType::String {
            format,
            min_length: None,
            max_length: None,
            nullable: false,
        }
    }

    #[test]
    fn secret_value_rendered_verbatim_from_server() {
        let schema = Schema::default();
        let cache = DisplayCache::new();
        let ansi = Ansi::new(false);
        let typ = string_type(StringFormat::Secret);

        let mut out = s();
        render_inline(
            &mut out,
            &schema,
            &json!("API_AAAADAAAAAJwz-sJIxu1a-wgtRpEAGDyzlJH7Q"),
            &typ,
            &cache,
            ansi,
        );
        assert_eq!(out, "API_AAAADAAAAAJwz-sJIxu1a-wgtRpEAGDyzlJH7Q");

        let mut out = s();
        render_inline(&mut out, &schema, &json!("****"), &typ, &cache, ansi);
        assert_eq!(out, "****");

        let mut out = s();
        render_inline(
            &mut out,
            &schema,
            &json!("-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----\n"),
            &string_type(StringFormat::SecretText),
            &cache,
            ansi,
        );
        assert_eq!(
            out,
            "-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----\n"
        );
    }

    #[test]
    fn size_units() {
        let mut out = s();
        render_size(&mut out, 0.0);
        assert_eq!(out, "0 B");
        let mut out = s();
        render_size(&mut out, 512.0);
        assert_eq!(out, "512 B");
        let mut out = s();
        render_size(&mut out, 1024.0);
        assert_eq!(out, "1.0 KB");
        let mut out = s();
        render_size(&mut out, 10.0 * 1024.0 * 1024.0);
        assert_eq!(out, "10.0 MB");
        let mut out = s();
        render_size(&mut out, 200.0 * 1024.0 * 1024.0);
        assert_eq!(out, "200 MB");
        let mut out = s();
        render_size(&mut out, 1024.0_f64.powi(4));
        assert_eq!(out, "1.0 TB");
    }

    #[test]
    fn duration_units() {
        let mut out = s();
        render_duration(&mut out, 0.0);
        assert_eq!(out, "0 ms");
        let mut out = s();
        render_duration(&mut out, 45.0);
        assert_eq!(out, "45 ms");
        let mut out = s();
        render_duration(&mut out, 5000.0);
        assert_eq!(out, "5 s");
        let mut out = s();
        render_duration(&mut out, 90_000.0);
        assert_eq!(out, "1 m 30 s");
        let mut out = s();
        render_duration(&mut out, (2 * 3600 + 15 * 60) as f64 * 1000.0);
        assert_eq!(out, "2 h 15 m");
        let mut out = s();
        render_duration(&mut out, (86_400 + 5 * 3600) as f64 * 1000.0);
        assert_eq!(out, "1 d 5 h");
    }
}
