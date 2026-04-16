/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::render::Ansi;
use crate::render::value::render_inline;
use crate::schema::{
    Field, FieldType, Fields, Form, FormField, MapValueType, ObjectSchema, ObjectType, Schema,
};
use crate::util::display_cache::DisplayCache;
use serde_json::{Map, Value};

fn find_variant_form<'a>(
    schema: &'a Schema,
    object_name: &str,
    variant_name: Option<&str>,
    schema_name: Option<&str>,
) -> Option<&'a Form> {
    if let Some(sn) = schema_name
        && let Some(f) = schema.forms.get(sn)
    {
        return Some(f);
    }
    if let Some(vn) = variant_name {
        let view_key = format!("{}/{}", object_name, vn);
        if let Some(f) = schema.forms.get(&view_key) {
            return Some(f);
        }
    }
    schema.forms.get(object_name)
}

fn find_form<'a>(schema: &'a Schema, object_name: &str, schema_name: &str) -> Option<&'a Form> {
    if let Some(f) = schema.forms.get(object_name) {
        return Some(f);
    }
    if let Some(f) = schema.forms.get(schema_name) {
        return Some(f);
    }
    for (view_key, entry) in &schema.objects {
        if let ObjectType::View {
            object_name: parent,
        } = entry
            && parent == object_name
            && let Some(f) = schema.forms.get(view_key)
        {
            return Some(f);
        }
    }
    None
}

pub struct RenderCtx<'a> {
    pub schema: &'a Schema,
    pub cache: &'a DisplayCache,
    pub ansi: Ansi,
}

pub fn render_top(out: &mut String, ctx: &RenderCtx, object_name: &str, obj: &Map<String, Value>) {
    let (fields, form, variant_label) = resolve_rendering(ctx.schema, object_name, obj);

    if let Some(label) = variant_label {
        out.push_str(ctx.ansi.bold());
        out.push_str("Type:");
        out.push_str(ctx.ansi.reset());
        out.push(' ');
        out.push_str(label);
        out.push_str("\n\n");
    }

    let Some(fields) = fields else {
        out.push_str(&serde_json::to_string_pretty(obj).unwrap_or_default());
        out.push('\n');
        return;
    };

    render_body(out, ctx, fields, form, obj, object_name, 0);
}

fn resolve_rendering<'a>(
    schema: &'a Schema,
    object_name: &str,
    obj: &Map<String, Value>,
) -> (Option<&'a Fields>, Option<&'a Form>, Option<&'a str>) {
    let Some(obj_schema) = schema.schemas.get(object_name) else {
        return (None, None, None);
    };
    match obj_schema {
        ObjectSchema::Single { schema_name } => {
            let fields = schema.fields.get(schema_name);
            let form = find_form(schema, object_name, schema_name);
            (fields, form, None)
        }
        ObjectSchema::Multiple { variants } => {
            let at_type = obj.get("@type").and_then(Value::as_str);
            let variant = at_type.and_then(|t| variants.iter().find(|v| v.name == t));
            let schema_name = variant.and_then(|v| v.schema_name.as_deref());
            let fields = schema_name.and_then(|n| schema.fields.get(n));

            let form = find_variant_form(
                schema,
                object_name,
                variant.map(|v| v.name.as_str()),
                schema_name,
            );
            (fields, form, variant.map(|v| v.label.as_str()))
        }
    }
}

fn render_body(
    out: &mut String,
    ctx: &RenderCtx,
    fields: &Fields,
    form: Option<&Form>,
    obj: &Map<String, Value>,
    object_name: &str,
    base_indent: usize,
) {
    type SectionItems<'a> = Vec<(&'a str, &'a str, &'a Field)>;

    let mut candidate_forms: Vec<&Form> = Vec::new();
    if let Some(f) = form {
        candidate_forms.push(f);
    }
    for (view_key, entry) in &ctx.schema.objects {
        if let ObjectType::View {
            object_name: parent,
        } = entry
            && parent == object_name
            && let Some(f) = ctx.schema.forms.get(view_key)
        {
            candidate_forms.push(f);
        }
    }

    let sections: Vec<(Option<&str>, SectionItems<'_>)> = if !candidate_forms.is_empty() {
        let mut out_secs: Vec<(Option<&str>, SectionItems<'_>)> = Vec::new();
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for form in &candidate_forms {
            for section in &form.sections {
                let mut items: SectionItems<'_> = Vec::with_capacity(section.fields.len());
                for ff in &section.fields {
                    if ff.name == "@type" {
                        continue;
                    }
                    let Some(field) = fields.properties.get(&ff.name) else {
                        continue;
                    };
                    if !obj.contains_key(&ff.name) {
                        continue;
                    }
                    if !seen.insert(ff.name.as_str()) {
                        continue;
                    }
                    items.push((label_of(ff), ff.name.as_str(), field));
                }
                if !items.is_empty() {
                    out_secs.push((section.title.as_deref(), items));
                }
            }
        }

        let mut orphans: SectionItems<'_> = Vec::new();
        for (name, field) in &fields.properties {
            if !obj.contains_key(name) {
                continue;
            }
            if seen.contains(name.as_str()) {
                continue;
            }
            orphans.push((name.as_str(), name.as_str(), field));
        }
        if !orphans.is_empty() {
            out_secs.push((None, orphans));
        }
        out_secs
    } else {
        let mut items: SectionItems<'_> = Vec::new();
        for (name, field) in &fields.properties {
            if !obj.contains_key(name) {
                continue;
            }
            items.push((name.as_str(), name.as_str(), field));
        }
        vec![(None, items)]
    };

    let mut first_section = true;
    for (title, items) in &sections {
        if items.is_empty() {
            continue;
        }
        if !first_section {
            out.push('\n');
        }
        first_section = false;
        if let Some(t) = title
            && !t.is_empty()
        {
            push_indent(out, base_indent);
            out.push_str(ctx.ansi.bold());
            out.push_str(t);
            out.push_str(ctx.ansi.reset());
            out.push('\n');
        }
        let label_width = items.iter().map(|(l, _, _)| l.len()).max().unwrap_or(0);

        for (label, name, field) in items {
            let value = match obj.get(*name) {
                Some(v) => v,
                None => continue,
            };
            render_field(out, ctx, label, label_width, field, value, base_indent);
        }
    }
}

fn render_field(
    out: &mut String,
    ctx: &RenderCtx,
    label: &str,
    label_width: usize,
    field: &Field,
    value: &Value,
    base_indent: usize,
) {
    push_indent(out, base_indent);

    out.push_str("  ");
    out.push_str(ctx.ansi.bold());
    out.push_str(label);
    out.push(':');
    out.push_str(ctx.ansi.reset());

    match &field.typ {
        FieldType::Object { object_name, .. } if !value.is_null() => {
            out.push('\n');
            render_nested_object(out, ctx, object_name, value, base_indent + 4);
        }
        FieldType::ObjectList { object_name, .. } if !value.is_null() => {
            out.push('\n');
            render_object_list_table(out, ctx, object_name, value, base_indent + 4);
        }
        FieldType::Map {
            key_class,
            value_class: MapValueType::Object { object_name },
            ..
        } if !value.is_null() => {
            out.push('\n');
            render_map_object_table(
                out,
                ctx,
                key_class,
                object_name,
                value,
                field,
                base_indent + 4,
            );
        }
        _ => {
            for _ in label.len()..label_width {
                out.push(' ');
            }
            out.push(' ');
            render_inline(out, ctx.schema, value, &field.typ, ctx.cache, ctx.ansi);
            out.push('\n');
        }
    }
}

fn render_nested_object(
    out: &mut String,
    ctx: &RenderCtx,
    object_name: &str,
    value: &Value,
    indent: usize,
) {
    let Some(obj) = value.as_object() else {
        push_indent(out, indent);
        out.push_str(&serde_json::to_string(value).unwrap_or_default());
        out.push('\n');
        return;
    };
    let (fields, form, variant_label) = resolve_rendering(ctx.schema, object_name, obj);

    if let Some(label) = variant_label {
        push_indent(out, indent);
        out.push_str(ctx.ansi.bold());
        out.push_str("Type:");
        out.push_str(ctx.ansi.reset());
        out.push(' ');
        out.push_str(label);
        out.push('\n');
    }
    let Some(fields) = fields else {
        if variant_label.is_none() {
            push_indent(out, indent);
            out.push_str(&serde_json::to_string(value).unwrap_or_default());
            out.push('\n');
        }
        return;
    };
    render_body(out, ctx, fields, form, obj, object_name, indent);
}

fn render_object_list_table(
    out: &mut String,
    ctx: &RenderCtx,
    object_name: &str,
    value: &Value,
    indent: usize,
) {
    let Some(map) = value.as_object() else {
        push_indent(out, indent);
        out.push_str(&serde_json::to_string(value).unwrap_or_default());
        out.push('\n');
        return;
    };
    if map.is_empty() {
        push_indent(out, indent);
        out.push_str(ctx.ansi.dim());
        out.push_str("<empty>");
        out.push_str(ctx.ansi.reset());
        out.push('\n');
        return;
    }

    let column_order = table_columns_for(ctx.schema, object_name);
    let Some(columns) = column_order else {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort_by(|a, b| natural_cmp(a, b));
        for k in keys {
            push_indent(out, indent);
            out.push_str(ctx.ansi.dim());
            out.push_str(k);
            out.push(':');
            out.push_str(ctx.ansi.reset());
            out.push('\n');
            if let Some(v) = map.get(k) {
                render_nested_object(out, ctx, object_name, v, indent + 2);
            }
        }
        return;
    };

    let rows: Vec<(&String, &Map<String, Value>)> = {
        let mut v: Vec<_> = map
            .iter()
            .filter_map(|(k, v)| v.as_object().map(|m| (k, m)))
            .collect();
        v.sort_by(|a, b| natural_cmp(a.0, b.0));
        v
    };

    render_table(out, ctx, "#", &columns, &rows, indent);
}

fn render_map_object_table(
    out: &mut String,
    ctx: &RenderCtx,
    key_class: &crate::schema::ScalarType,
    object_name: &str,
    value: &Value,
    parent_field: &Field,
    indent: usize,
) {
    let _ = parent_field;
    let Some(map) = value.as_object() else {
        push_indent(out, indent);
        out.push_str(&serde_json::to_string(value).unwrap_or_default());
        out.push('\n');
        return;
    };
    if map.is_empty() {
        push_indent(out, indent);
        out.push_str(ctx.ansi.dim());
        out.push_str("<empty>");
        out.push_str(ctx.ansi.reset());
        out.push('\n');
        return;
    }

    let column_order = table_columns_for(ctx.schema, object_name);
    let Some(columns) = column_order else {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        for k in keys {
            push_indent(out, indent);
            out.push_str(ctx.ansi.dim());
            out.push_str(k);
            out.push(':');
            out.push_str(ctx.ansi.reset());
            out.push('\n');
            if let Some(v) = map.get(k) {
                render_nested_object(out, ctx, object_name, v, indent + 2);
            }
        }
        return;
    };

    let mut rows: Vec<(String, &Map<String, Value>)> = Vec::with_capacity(map.len());
    for (k, v) in map {
        if let Some(inner) = v.as_object() {
            let rendered_key = render_key_for_table(ctx, key_class, k);
            rows.push((rendered_key, inner));
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let rows: Vec<(&String, &Map<String, Value>)> = rows.iter().map(|(k, v)| (k, *v)).collect();

    render_table(out, ctx, "Key", &columns, &rows, indent);
}

fn render_key_for_table(
    ctx: &RenderCtx,
    key_class: &crate::schema::ScalarType,
    key: &str,
) -> String {
    use crate::schema::ScalarType;
    let mut out = String::new();
    match key_class {
        ScalarType::String { .. } => out.push_str(key),
        ScalarType::ObjectId { object_name } => match ctx.cache.get(object_name, key) {
            Some(label) => {
                out.push_str(label);
                out.push_str(" (id: ");
                out.push_str(key);
                out.push(')');
            }
            None => out.push_str(key),
        },
        ScalarType::Enum { enum_name } => {
            let v = ctx
                .schema
                .enums
                .get(enum_name)
                .and_then(|vs| vs.iter().find(|v| v.name == key));
            match v {
                Some(v) => out.push_str(&v.label),
                None => out.push_str(key),
            }
        }
    }
    out
}

struct TableColumn {
    label: String,
    name: String,
    field_type: FieldType,
}

fn table_columns_for(schema: &Schema, object_name: &str) -> Option<Vec<TableColumn>> {
    let obj_schema = schema.schemas.get(object_name)?;
    let schema_name = match obj_schema {
        ObjectSchema::Single { schema_name } => schema_name.as_str(),
        ObjectSchema::Multiple { .. } => return None,
    };
    let fields = schema.fields.get(schema_name)?;
    let form = find_form(schema, object_name, schema_name);

    let mut columns = Vec::new();
    if let Some(form) = form {
        for section in &form.sections {
            for ff in &section.fields {
                if ff.name == "@type" {
                    continue;
                }
                if let Some(field) = fields.properties.get(&ff.name) {
                    if is_composite(&field.typ) {
                        continue;
                    }
                    columns.push(TableColumn {
                        label: label_of(ff).to_string(),
                        name: ff.name.clone(),
                        field_type: field.typ.clone(),
                    });
                }
            }
        }
    } else {
        for (name, field) in &fields.properties {
            if is_composite(&field.typ) {
                continue;
            }
            columns.push(TableColumn {
                label: name.clone(),
                name: name.clone(),
                field_type: field.typ.clone(),
            });
        }
    }
    if columns.is_empty() {
        None
    } else {
        Some(columns)
    }
}

fn is_composite(t: &FieldType) -> bool {
    matches!(
        t,
        FieldType::Object { .. }
            | FieldType::ObjectList { .. }
            | FieldType::Map {
                value_class: MapValueType::Object { .. },
                ..
            }
    )
}

fn render_table(
    out: &mut String,
    ctx: &RenderCtx,
    first_header: &str,
    columns: &[TableColumn],
    rows: &[(&String, &Map<String, Value>)],
    indent: usize,
) {
    let mut widths: Vec<usize> = std::iter::once(first_header.len())
        .chain(columns.iter().map(|c| c.label.len()))
        .collect();

    let mut rendered_rows: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    for (key, obj) in rows {
        let mut row = Vec::with_capacity(columns.len() + 1);
        row.push((*key).clone());
        widths[0] = widths[0].max(row[0].len());
        for (i, col) in columns.iter().enumerate() {
            let mut cell = String::new();
            if let Some(v) = obj.get(&col.name) {
                render_inline(
                    &mut cell,
                    ctx.schema,
                    v,
                    &col.field_type,
                    ctx.cache,
                    Ansi::new(false),
                );
            }
            widths[i + 1] = widths[i + 1].max(cell.len());
            row.push(cell);
        }
        rendered_rows.push(row);
    }

    push_indent(out, indent);
    out.push_str(ctx.ansi.bold());
    push_padded(out, first_header, widths[0]);
    for (i, c) in columns.iter().enumerate() {
        out.push_str("  ");
        push_padded(out, &c.label, widths[i + 1]);
    }
    out.push_str(ctx.ansi.reset());
    out.push('\n');

    for row in &rendered_rows {
        push_indent(out, indent);
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                out.push_str("  ");
            }
            push_padded(out, cell, widths[i]);
        }
        out.push('\n');
    }
}

fn push_padded(out: &mut String, s: &str, width: usize) {
    out.push_str(s);
    for _ in s.len()..width {
        out.push(' ');
    }
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
}

fn label_of(ff: &FormField) -> &str {
    if ff.label.is_empty() {
        &ff.name
    } else {
        &ff.label
    }
}

fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        _ => a.cmp(b),
    }
}
