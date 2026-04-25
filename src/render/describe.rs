/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::render::Ansi;
use crate::schema::resolve::display_name;
use crate::schema::{
    FieldType, FieldUpdate, Filter, MapValueType, NumberFormat, ObjectSchema, ObjectType,
    ScalarType, Schema, StringFormat,
};
use std::collections::HashSet;

pub fn list_all(schema: &Schema, ansi: Ansi) -> String {
    let mut entries: Vec<(&str, bool, &str)> = schema
        .objects
        .iter()
        .filter_map(|(k, v)| {
            if !k.starts_with("x:") {
                return None;
            }
            match v {
                ObjectType::Object { description, .. } => {
                    Some((k.as_str(), false, description.as_str()))
                }
                ObjectType::Singleton { description, .. } => {
                    Some((k.as_str(), true, description.as_str()))
                }
                ObjectType::View { .. } => None,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let name_width = entries
        .iter()
        .map(|(k, _, _)| display_name(k).len())
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    for (name, is_singleton, desc) in entries {
        let disp = display_name(name);
        out.push_str(ansi.cyan());
        out.push_str(disp);
        out.push_str(ansi.reset());
        for _ in disp.len()..name_width {
            out.push(' ');
        }
        out.push_str("  ");
        out.push_str(desc);
        if is_singleton {
            out.push(' ');
            out.push_str(ansi.dim());
            out.push_str("[singleton]");
            out.push_str(ansi.reset());
        }
        out.push('\n');
    }
    out
}

pub fn describe_object(schema: &Schema, canonical: &str, ansi: Ansi) -> String {
    let mut out = String::new();
    let entry = match schema.objects.get(canonical) {
        Some(e) => e,
        None => {
            out.push_str("unknown object\n");
            return out;
        }
    };

    let (description, is_singleton) = match entry {
        ObjectType::Object { description, .. } => (description.as_str(), false),
        ObjectType::Singleton { description, .. } => (description.as_str(), true),
        ObjectType::View { .. } => return out,
    };

    out.push_str(ansi.bold());
    out.push_str(display_name(canonical));
    out.push_str(ansi.reset());
    if is_singleton {
        out.push(' ');
        out.push_str(ansi.dim());
        out.push_str("[singleton]");
        out.push_str(ansi.reset());
    }
    out.push('\n');
    out.push_str("  ");
    out.push_str(description);
    out.push_str("\n\n");

    let obj_schema = match schema.schemas.get(canonical) {
        Some(s) => s,
        None => return out,
    };

    match obj_schema {
        ObjectSchema::Single { schema_name } => {
            if let Some(fields) = schema.fields.get(schema_name) {
                render_fields_section(&mut out, "Fields:", fields, ansi);
            }
        }
        ObjectSchema::Multiple { variants } => {
            out.push_str(ansi.bold());
            out.push_str("  Variants:");
            out.push_str(ansi.reset());
            out.push('\n');
            for v in variants {
                out.push_str("    ");
                out.push_str(ansi.cyan());
                out.push_str(&v.name);
                out.push_str(ansi.reset());
                out.push_str(": ");
                out.push_str(&v.label);
                out.push('\n');
                if let Some(schema_name) = &v.schema_name
                    && let Some(fields) = schema.fields.get(schema_name)
                {
                    render_fields_section(&mut out, "      Fields:", fields, ansi);
                }
            }
        }
    }

    let filters = collect_filters(schema, canonical);
    if !filters.is_empty() {
        out.push('\n');
        render_filters_section(&mut out, &filters, ansi);
    }
    let sort = collect_sort(schema, canonical);
    if !sort.is_empty() {
        out.push('\n');
        render_sort_section(&mut out, &sort, ansi);
    }

    out
}

fn collect_filters<'a>(schema: &'a Schema, object_name: &str) -> Vec<&'a Filter> {
    let mut out: Vec<&'a Filter> = Vec::new();
    let mut seen: HashSet<&'a str> = HashSet::new();
    let add = |list_key: &str, out: &mut Vec<&'a Filter>, seen: &mut HashSet<&'a str>| {
        if let Some(list) = schema.lists.get(list_key) {
            for f in &list.filters {
                let field = filter_field(f);
                if seen.insert(field) {
                    out.push(f);
                }
            }
        }
    };
    add(object_name, &mut out, &mut seen);
    for (view_key, entry) in &schema.objects {
        if let ObjectType::View {
            object_name: parent,
        } = entry
            && parent == object_name
        {
            add(view_key, &mut out, &mut seen);
        }
    }
    out
}

fn collect_sort<'a>(schema: &'a Schema, object_name: &str) -> Vec<&'a str> {
    let mut out: Vec<&'a str> = Vec::new();
    let mut seen: HashSet<&'a str> = HashSet::new();
    let add = |list_key: &str, out: &mut Vec<&'a str>, seen: &mut HashSet<&'a str>| {
        if let Some(list) = schema.lists.get(list_key) {
            for s in &list.sort {
                if seen.insert(s.as_str()) {
                    out.push(s.as_str());
                }
            }
        }
    };
    add(object_name, &mut out, &mut seen);
    for (view_key, entry) in &schema.objects {
        if let ObjectType::View {
            object_name: parent,
        } = entry
            && parent == object_name
        {
            add(view_key, &mut out, &mut seen);
        }
    }
    out
}

fn filter_field(f: &Filter) -> &str {
    match f {
        Filter::Text { field, .. }
        | Filter::Enum { field, .. }
        | Filter::Integer { field, .. }
        | Filter::Date { field, .. }
        | Filter::ObjectId { field, .. } => field.as_str(),
    }
}

fn filter_kind(f: &Filter) -> String {
    match f {
        Filter::Text { .. } => "text".to_string(),
        Filter::Integer { .. } => "integer".to_string(),
        Filter::Date { .. } => "date".to_string(),
        Filter::Enum { enum_name, .. } => format!("enum (see {enum_name})"),
        Filter::ObjectId { object_name, .. } => format!("id<{}>", display_name(object_name)),
    }
}

fn filter_label(f: &Filter) -> &str {
    match f {
        Filter::Text { label, .. }
        | Filter::Enum { label, .. }
        | Filter::Integer { label, .. }
        | Filter::Date { label, .. }
        | Filter::ObjectId { label, .. } => label.as_str(),
    }
}

fn render_filters_section(out: &mut String, filters: &[&Filter], ansi: Ansi) {
    out.push_str(ansi.bold());
    out.push_str("Filters:");
    out.push_str(ansi.reset());
    out.push('\n');

    let name_width = filters
        .iter()
        .map(|f| filter_field(f).len())
        .max()
        .unwrap_or(0);
    let kind_strings: Vec<String> = filters.iter().map(|f| filter_kind(f)).collect();
    let kind_width = kind_strings.iter().map(|s| s.len()).max().unwrap_or(0);

    for (f, kind) in filters.iter().zip(kind_strings.iter()) {
        let name = filter_field(f);
        out.push_str("    ");
        out.push_str(ansi.cyan());
        out.push_str(name);
        out.push_str(ansi.reset());
        for _ in name.len()..name_width {
            out.push(' ');
        }
        out.push_str("  ");
        out.push_str(kind);
        for _ in kind.len()..kind_width {
            out.push(' ');
        }
        out.push_str("  ");
        out.push_str(filter_label(f));
        out.push('\n');
    }
}

fn render_sort_section(out: &mut String, sort: &[&str], ansi: Ansi) {
    out.push_str(ansi.bold());
    out.push_str("Sort by:");
    out.push_str(ansi.reset());
    out.push('\n');
    out.push_str("    ");
    let mut first = true;
    for field in sort {
        if !first {
            out.push_str(", ");
        }
        first = false;
        out.push_str(ansi.cyan());
        out.push_str(field);
        out.push_str(ansi.reset());
    }
    out.push('\n');
}

fn render_fields_section(
    out: &mut String,
    header: &str,
    fields: &crate::schema::Fields,
    ansi: Ansi,
) {
    out.push_str(ansi.bold());
    out.push_str(header);
    out.push_str(ansi.reset());
    out.push('\n');
    let indent_header = if header.starts_with("      ") {
        "        "
    } else {
        "    "
    };
    let indent_desc = if header.starts_with("      ") {
        "            "
    } else {
        "        "
    };

    let mut names: Vec<&String> = fields.properties.keys().collect();
    names.sort();
    let name_width = names.iter().map(|n| n.len()).max().unwrap_or(0);

    for name in names {
        let field = &fields.properties[name];
        out.push_str(indent_header);
        out.push_str(ansi.cyan());
        out.push_str(name);
        out.push_str(ansi.reset());
        for _ in name.len()..name_width {
            out.push(' ');
        }
        out.push_str("  ");
        push_type_summary(out, &field.typ);
        out.push_str("  ");
        out.push_str(ansi.dim());
        out.push_str(update_mode_str(&field.update));
        out.push_str(ansi.reset());
        out.push('\n');
        if !field.description.is_empty() {
            for line in field.description.lines() {
                out.push_str(indent_desc);
                out.push_str(line);
                out.push('\n');
            }
        }
    }
}

fn update_mode_str(u: &FieldUpdate) -> &'static str {
    match u {
        FieldUpdate::Mutable => "mutable",
        FieldUpdate::Immutable => "immutable",
        FieldUpdate::ServerSet => "server-set",
    }
}

pub fn push_type_summary(out: &mut String, t: &FieldType) {
    match t {
        FieldType::String {
            format, nullable, ..
        } => {
            out.push_str("string<");
            out.push_str(string_format_name(format));
            out.push('>');
            if *nullable {
                out.push('?');
            }
        }
        FieldType::Number {
            format, nullable, ..
        } => {
            out.push_str("number<");
            out.push_str(number_format_name(format));
            out.push('>');
            if *nullable {
                out.push('?');
            }
        }
        FieldType::UtcDateTime { nullable } => {
            out.push_str("datetime");
            if *nullable {
                out.push('?');
            }
        }
        FieldType::Boolean => out.push_str("boolean"),
        FieldType::Enum {
            enum_name,
            nullable,
        } => {
            out.push_str("enum (see ");
            out.push_str(enum_name);
            out.push(')');
            if *nullable {
                out.push('?');
            }
        }
        FieldType::BlobId => out.push_str("blob"),
        FieldType::ObjectId {
            object_name,
            nullable,
        } => {
            out.push_str("id<");
            out.push_str(display_name(object_name));
            out.push('>');
            if *nullable {
                out.push('?');
            }
        }
        FieldType::Object {
            object_name,
            nullable,
        } => {
            out.push_str("object<");
            out.push_str(display_name(object_name));
            out.push('>');
            if *nullable {
                out.push('?');
            }
        }
        FieldType::ObjectList { object_name, .. } => {
            out.push_str("list<");
            out.push_str(display_name(object_name));
            out.push('>');
        }
        FieldType::Set { class, .. } => {
            out.push_str("set<");
            push_scalar_summary(out, class);
            out.push('>');
        }
        FieldType::Map {
            key_class,
            value_class,
            ..
        } => {
            out.push_str("map<");
            push_scalar_summary(out, key_class);
            out.push_str(", ");
            push_map_value_summary(out, value_class);
            out.push('>');
        }
    }
}

fn push_scalar_summary(out: &mut String, c: &ScalarType) {
    match c {
        ScalarType::String { format, .. } => {
            out.push_str("string<");
            out.push_str(string_format_name(format));
            out.push('>');
        }
        ScalarType::ObjectId { object_name } => {
            out.push_str("id<");
            out.push_str(display_name(object_name));
            out.push('>');
        }
        ScalarType::Enum { enum_name } => {
            out.push_str("enum (see ");
            out.push_str(enum_name);
            out.push(')');
        }
    }
}

fn push_map_value_summary(out: &mut String, c: &MapValueType) {
    match c {
        MapValueType::String { format, .. } => {
            out.push_str("string<");
            out.push_str(string_format_name(format));
            out.push('>');
        }
        MapValueType::Number { format, .. } => {
            out.push_str("number<");
            out.push_str(number_format_name(format));
            out.push('>');
        }
        MapValueType::Enum { enum_name } => {
            out.push_str("enum (see ");
            out.push_str(enum_name);
            out.push(')');
        }
        MapValueType::Object { object_name } => {
            out.push_str("object<");
            out.push_str(display_name(object_name));
            out.push('>');
        }
    }
}

fn string_format_name(f: &StringFormat) -> &'static str {
    match f {
        StringFormat::String => "string",
        StringFormat::IpAddress => "ipAddress",
        StringFormat::IpNetwork => "ipNetwork",
        StringFormat::SocketAddress => "socketAddress",
        StringFormat::EmailAddress => "emailAddress",
        StringFormat::Secret => "secret",
        StringFormat::SecretText => "secretText",
        StringFormat::Uri => "uri",
        StringFormat::Color => "color",
        StringFormat::Text => "text",
        StringFormat::Html => "html",
    }
}

fn number_format_name(f: &NumberFormat) -> &'static str {
    match f {
        NumberFormat::Integer => "integer",
        NumberFormat::UnsignedInteger => "unsignedInteger",
        NumberFormat::Float => "float",
        NumberFormat::Size => "size",
        NumberFormat::Duration => "duration",
    }
}

pub fn describe_enum(schema: &Schema, canonical: &str, ansi: Ansi) -> String {
    let mut out = String::new();
    let variants = match schema.enums.get(canonical) {
        Some(v) => v,
        None => return out,
    };
    out.push_str(ansi.bold());
    out.push_str(canonical);
    out.push_str(ansi.reset());
    out.push_str("  ");
    out.push_str(ansi.dim());
    out.push_str("(enum)");
    out.push_str(ansi.reset());
    out.push_str("\n\n");

    out.push_str(ansi.bold());
    out.push_str("  Variants:");
    out.push_str(ansi.reset());
    out.push('\n');

    let name_width = variants.iter().map(|v| v.name.len()).max().unwrap_or(0);
    for v in variants {
        out.push_str("    ");
        if let Some(color) = &v.color {
            out.push_str(&ansi.named(color));
        } else {
            out.push_str(ansi.cyan());
        }
        out.push_str(&v.name);
        out.push_str(ansi.reset());
        for _ in v.name.len()..name_width {
            out.push(' ');
        }
        out.push_str("  ");
        out.push_str(&v.label);
        out.push('\n');
        if let Some(expl) = &v.explanation {
            for line in expl.lines() {
                out.push_str("        ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

pub fn unknown_describe_target(name: &str) -> String {
    let mut out = String::new();
    out.push_str("unknown object or enum: ");
    out.push_str(name);
    out.push('\n');
    out
}
