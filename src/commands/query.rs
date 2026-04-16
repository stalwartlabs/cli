/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::context::Context;
use crate::app::error::{CliError, CliResult};
use crate::cli::QueryArgs;
use crate::jmap::{Jmap, check_response};
use crate::render::Ansi;
use crate::render::value::render_inline;
use crate::schema::resolve;
use crate::schema::{
    Field, FieldType, Fields, List, NumberFormat, ObjectSchema, ObjectType, Schema,
};
use crate::util::display_cache::DisplayCache;
use is_terminal::IsTerminal;
use serde_json::{Map, Value, json};
use std::io::{BufRead, Write};

const PAGE_SIZE: usize = 10;
const MAX_CELL_WIDTH: usize = 60;

pub fn run(ctx: &Context, args: &QueryArgs) -> CliResult<()> {
    resolve::forbid_slash_form(&args.object)?;
    let canonical = resolve::require_object(&ctx.schema, &args.object)?;
    if matches!(
        ctx.schema.objects.get(canonical),
        Some(ObjectType::Singleton { .. })
    ) {
        return Err(CliError::msg(format!(
            "{} is a singleton and does not support query",
            resolve::display_name(canonical)
        )));
    }

    let union = union_fields(&ctx.schema, canonical);
    let filter = build_filter(&union, &args.wheres)?;

    let columns = resolve_columns(&ctx.schema, canonical, &union, args.fields.as_deref())?;

    let jmap = Jmap::new(&ctx.client, &ctx.session.api_path);
    let limit = ctx.session.max_objects_in_get.max(1);

    if args.json {
        let props = columns
            .as_ref()
            .map(|cs| cs.iter().map(|c| c.name.clone()).collect::<Vec<_>>());
        return run_json(ctx, &jmap, canonical, &filter, props.as_deref(), limit);
    }

    match columns {
        None => run_ids(ctx, &jmap, canonical, &filter, limit),
        Some(cols) => run_table(ctx, &jmap, canonical, &filter, &cols, &union, limit),
    }
}

pub struct Column {
    pub name: String,
    pub label: String,
    pub typ: Option<FieldType>,
}

fn resolve_columns(
    schema: &Schema,
    canonical: &str,
    union: &Fields,
    user_fields: Option<&[String]>,
) -> CliResult<Option<Vec<Column>>> {
    match user_fields {
        Some(raws) => {
            let mut out = Vec::with_capacity(raws.len());
            for raw in raws {
                let raw = raw.trim();
                let (name, typ) = if raw.eq_ignore_ascii_case("id") {
                    ("id".to_string(), None)
                } else if raw.eq_ignore_ascii_case("@type") {
                    ("@type".to_string(), None)
                } else {
                    match find_field(union, raw) {
                        Some((canon, f)) => (canon.to_string(), Some(f.typ.clone())),
                        None => return Err(CliError::UnknownField(raw.to_string())),
                    }
                };
                let label = label_for_field(schema, canonical, &name)
                    .unwrap_or(&name)
                    .to_string();
                out.push(Column { name, label, typ });
            }
            Ok(Some(out))
        }
        None => {
            let Some(list) = default_list_for(schema, canonical) else {
                return Ok(None);
            };

            let mut out = Vec::with_capacity(list.columns.len() + 1);
            out.push(Column {
                name: "id".to_string(),
                label: "Id".to_string(),
                typ: None,
            });
            for col in &list.columns {
                if col.name == "id" {
                    continue;
                }
                out.push(Column {
                    typ: union.properties.get(&col.name).map(|f| f.typ.clone()),
                    name: col.name.clone(),
                    label: col.label.clone(),
                });
            }
            Ok(Some(out))
        }
    }
}

fn label_for_field<'a>(schema: &'a Schema, object_name: &str, field_name: &str) -> Option<&'a str> {
    if let Some(l) = form_field_label(schema, object_name, field_name) {
        return Some(l);
    }

    if let Some(obj_schema) = schema.schemas.get(object_name) {
        match obj_schema {
            ObjectSchema::Single { schema_name } => {
                if let Some(l) = form_field_label(schema, schema_name, field_name) {
                    return Some(l);
                }
            }
            ObjectSchema::Multiple { variants } => {
                for v in variants {
                    if let Some(sn) = &v.schema_name
                        && let Some(l) = form_field_label(schema, sn, field_name)
                    {
                        return Some(l);
                    }
                }
            }
        }
    }

    for (view_key, entry) in &schema.objects {
        if let ObjectType::View {
            object_name: parent,
        } = entry
            && parent == object_name
            && let Some(l) = form_field_label(schema, view_key, field_name)
        {
            return Some(l);
        }
    }

    if let Some(list) = schema.lists.get(object_name)
        && let Some(c) = list.columns.iter().find(|c| c.name == field_name)
    {
        return Some(&c.label);
    }
    for (view_key, entry) in &schema.objects {
        if let ObjectType::View {
            object_name: parent,
        } = entry
            && parent == object_name
            && let Some(list) = schema.lists.get(view_key)
            && let Some(c) = list.columns.iter().find(|c| c.name == field_name)
        {
            return Some(&c.label);
        }
    }
    None
}

fn form_field_label<'a>(schema: &'a Schema, form_key: &str, field_name: &str) -> Option<&'a str> {
    let form = schema.forms.get(form_key)?;
    for section in &form.sections {
        for ff in &section.fields {
            if ff.name == field_name && !ff.label.is_empty() {
                return Some(&ff.label);
            }
        }
    }
    None
}

fn default_list_for<'a>(schema: &'a Schema, object_name: &str) -> Option<&'a List> {
    if let Some(l) = schema.lists.get(object_name) {
        return Some(l);
    }
    for (view_key, entry) in &schema.objects {
        if let ObjectType::View {
            object_name: parent,
        } = entry
            && parent == object_name
            && let Some(l) = schema.lists.get(view_key)
        {
            return Some(l);
        }
    }
    None
}

#[derive(Clone, Copy)]
enum Op {
    Eq,
    Gt,
    Gte,
    Lt,
    Lte,
}
impl Op {
    fn suffix(self) -> &'static str {
        match self {
            Op::Eq => "",
            Op::Gt => "IsGreaterThan",
            Op::Gte => "IsGreaterThanOrEqual",
            Op::Lt => "IsLessThan",
            Op::Lte => "IsLessThanOrEqual",
        }
    }
    fn sym(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Gt => ">",
            Op::Gte => ">=",
            Op::Lt => "<",
            Op::Lte => "<=",
        }
    }
}

fn parse_where(s: &str) -> CliResult<(&str, Op, &str)> {
    for (tok, op) in [
        (">=", Op::Gte),
        ("<=", Op::Lte),
        (">", Op::Gt),
        ("<", Op::Lt),
        ("=", Op::Eq),
    ] {
        if let Some(i) = s.find(tok) {
            let (lhs, rest) = s.split_at(i);
            let rhs = &rest[tok.len()..];
            if !lhs.is_empty() {
                return Ok((lhs.trim(), op, rhs));
            }
        }
    }
    Err(CliError::msg(format!(
        "invalid --where `{s}`: expected field=value (or field>=value, <=, >, <)"
    )))
}

fn build_filter(fields: &Fields, wheres: &[String]) -> CliResult<Value> {
    let mut out = Map::new();
    for w in wheres {
        let (key_raw, op, value_raw) = parse_where(w)?;
        let matched = find_field(fields, key_raw);
        if !matches!(op, Op::Eq)
            && let Some((_, field)) = matched
            && !matches!(
                field.typ,
                FieldType::Number { .. } | FieldType::UtcDateTime { .. }
            )
        {
            return Err(CliError::msg(format!(
                "operator `{}` is only allowed on number or datetime fields",
                op.sym()
            )));
        }
        let (key_canon, field_type) = match matched {
            Some((name, f)) => (name.to_string(), Some(&f.typ)),
            None => (key_raw.to_string(), None),
        };
        let coerced = coerce(value_raw, field_type)?;
        let key = format!("{}{}", key_canon, op.suffix());
        out.insert(key, coerced);
    }
    Ok(Value::Object(out))
}

fn coerce(raw: &str, ft: Option<&FieldType>) -> CliResult<Value> {
    match ft {
        Some(FieldType::Boolean) => match raw.to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => Ok(Value::Bool(true)),
            "false" | "no" | "0" => Ok(Value::Bool(false)),
            _ => Err(CliError::msg(format!("expected boolean, got `{raw}`"))),
        },
        Some(FieldType::Number {
            format: NumberFormat::Float,
            ..
        }) => raw
            .parse::<f64>()
            .map_err(|_| CliError::msg(format!("expected number, got `{raw}`")))
            .and_then(|n| {
                serde_json::Number::from_f64(n)
                    .map(Value::Number)
                    .ok_or_else(|| CliError::msg(format!("non-finite number `{raw}`")))
            }),
        Some(FieldType::Number { .. }) => raw
            .parse::<i64>()
            .map(|n| Value::Number(n.into()))
            .map_err(|_| CliError::msg(format!("expected integer, got `{raw}`"))),
        _ => Ok(Value::String(raw.to_string())),
    }
}

fn find_field<'a>(fields: &'a Fields, raw: &str) -> Option<(&'a str, &'a Field)> {
    fields
        .properties
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(raw))
        .map(|(k, f)| (k.as_str(), f))
}

fn union_fields(schema: &Schema, canonical: &str) -> Fields {
    let Some(obj_schema) = schema.schemas.get(canonical) else {
        return Fields::default();
    };
    match obj_schema {
        ObjectSchema::Single { schema_name } => {
            schema.fields.get(schema_name).cloned().unwrap_or_default()
        }
        ObjectSchema::Multiple { variants } => {
            let mut out = Fields::default();
            for v in variants {
                if let Some(sn) = &v.schema_name
                    && let Some(f) = schema.fields.get(sn)
                {
                    for (k, field) in &f.properties {
                        out.properties
                            .entry(k.clone())
                            .or_insert_with(|| field.clone());
                    }
                }
            }
            out
        }
    }
}

struct Pager {
    limit: usize,
    anchor: Option<String>,
}

impl Pager {
    fn new(limit: usize) -> Self {
        Pager {
            limit,
            anchor: None,
        }
    }
    fn advance(&mut self, last_id: String) {
        self.anchor = Some(last_id);
    }
    fn query_args(&self, filter: &Value) -> Value {
        let mut args = Map::new();
        args.insert("filter".to_string(), filter.clone());
        args.insert("limit".to_string(), Value::from(self.limit));
        if let Some(a) = &self.anchor {
            args.insert("anchor".to_string(), Value::String(a.clone()));
            args.insert("anchorOffset".to_string(), Value::from(1));
        } else {
            args.insert("calculateTotal".to_string(), Value::Bool(true));
        }
        Value::Object(args)
    }
}

fn fetch_ids(
    jmap: &Jmap,
    canonical: &str,
    filter: &Value,
    pager: &Pager,
) -> CliResult<Vec<String>> {
    let method = format!("{canonical}/query");
    let result = jmap.call(&method, pager.query_args(filter))?;
    Ok(result
        .get("ids")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

fn fetch_rows(
    jmap: &Jmap,
    canonical: &str,
    filter: &Value,
    pager: &Pager,
    properties: &[String],
) -> CliResult<Vec<Map<String, Value>>> {
    let query_method = format!("{canonical}/query");
    let get_method = format!("{canonical}/get");
    let calls = vec![
        (
            query_method.clone(),
            pager.query_args(filter),
            "q".to_string(),
        ),
        (
            get_method.clone(),
            json!({
                "#ids": { "resultOf": "q", "name": query_method, "path": "/ids" },
                "properties": properties,
            }),
            "g".to_string(),
        ),
    ];
    let mut responses = jmap.call_many(calls)?;
    if responses.len() != 2 {
        return Err(CliError::UnexpectedResponse(format!(
            "expected 2 responses, got {}",
            responses.len()
        )));
    }

    let g = responses
        .pop()
        .ok_or_else(|| CliError::UnexpectedResponse("missing get response".into()))?;
    let q = responses
        .pop()
        .ok_or_else(|| CliError::UnexpectedResponse("missing query response".into()))?;
    check_response(q, &query_method)?;
    let g_result = check_response(g, &get_method)?;

    let list = g_result
        .get("list")
        .and_then(Value::as_array)
        .ok_or_else(|| CliError::UnexpectedResponse("get missing 'list'".into()))?;
    Ok(list.iter().filter_map(|v| v.as_object().cloned()).collect())
}

fn run_ids(
    _ctx: &Context,
    jmap: &Jmap,
    canonical: &str,
    filter: &Value,
    limit: usize,
) -> CliResult<()> {
    let is_tty = std::io::stdout().is_terminal();
    let mut pager = Pager::new(limit);
    let mut displayed = 0usize;
    let mut stdout = std::io::stdout().lock();

    loop {
        let ids = fetch_ids(jmap, canonical, filter, &pager)?;
        if ids.is_empty() {
            break;
        }
        let last_page = ids.len() < limit;

        for id in &ids {
            writeln!(stdout, "{id}")?;
            displayed += 1;
            if is_tty && displayed.is_multiple_of(PAGE_SIZE) && !last_page {
                drop(stdout);
                if !prompt_more()? {
                    return Ok(());
                }
                stdout = std::io::stdout().lock();
            }
        }

        if last_page {
            break;
        }
        if let Some(last) = ids.last() {
            pager.advance(last.clone());
        } else {
            break;
        }
        if is_tty && displayed > 0 && !displayed.is_multiple_of(PAGE_SIZE) {
            drop(stdout);
            if !prompt_more()? {
                return Ok(());
            }
            stdout = std::io::stdout().lock();
        }
    }
    Ok(())
}

fn run_table(
    ctx: &Context,
    jmap: &Jmap,
    canonical: &str,
    filter: &Value,
    columns: &[Column],
    union: &Fields,
    limit: usize,
) -> CliResult<()> {
    let is_tty = std::io::stdout().is_terminal();
    let ansi = Ansi::new(ctx.config.color);
    let mut pager = Pager::new(limit);
    let mut displayed = 0usize;
    let properties: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();

    loop {
        let rows = fetch_rows(jmap, canonical, filter, &pager, &properties)?;
        if rows.is_empty() {
            break;
        }
        let last_page = rows.len() < limit;

        let mut cache = DisplayCache::new();
        let refs: Vec<&Map<String, Value>> = rows.iter().collect();
        cache.populate_from_objects(jmap, &ctx.schema, union, &refs)?;

        let cells: Vec<Vec<String>> = rows
            .iter()
            .map(|row| render_row_cells(columns, row, &ctx.schema, &cache, ansi))
            .collect();
        let widths: Vec<usize> = (0..columns.len())
            .map(|i| {
                cells
                    .iter()
                    .map(|r| visible_width(&r[i]))
                    .chain(std::iter::once(columns[i].label.len()))
                    .max()
                    .unwrap_or(columns[i].label.len())
            })
            .collect();

        let keep_going = emit_table_rows(
            columns,
            &widths,
            &cells,
            ansi,
            &mut displayed,
            is_tty,
            last_page,
        )?;
        if !keep_going {
            return Ok(());
        }

        if last_page {
            break;
        }
        if let Some(last) = rows
            .last()
            .and_then(|m| m.get("id").and_then(Value::as_str))
        {
            pager.advance(last.to_string());
        } else {
            break;
        }
    }
    Ok(())
}

fn render_row_cells(
    cols: &[Column],
    row: &Map<String, Value>,
    schema: &Schema,
    cache: &DisplayCache,
    ansi: Ansi,
) -> Vec<String> {
    cols.iter()
        .map(|col| {
            let mut s = String::new();
            if let Some(v) = row.get(&col.name) {
                match &col.typ {
                    Some(ft) => render_inline(&mut s, schema, v, ft, cache, ansi),
                    None => match v.as_str() {
                        Some(st) => s.push_str(st),
                        None => s.push_str(&serde_json::to_string(v).unwrap_or_default()),
                    },
                }
            }
            tidy_cell(&s)
        })
        .collect()
}

fn tidy_cell(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    let mut visible = 0usize;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            out.push(c);
            if c == 'm' {
                in_escape = false;
            }
            continue;
        }
        if c == '\x1b' {
            out.push(c);
            in_escape = true;
            continue;
        }
        if c == '\n' || c == '\r' || c == '\t' {
            if !prev_space {
                out.push(' ');
                visible += 1;
                prev_space = true;
            }
        } else {
            out.push(c);
            visible += 1;
            prev_space = false;
        }
        if visible >= MAX_CELL_WIDTH {
            out.push('…');
            break;
        }
    }
    out
}

fn emit_table_rows(
    columns: &[Column],
    widths: &[usize],
    cells: &[Vec<String>],
    ansi: Ansi,
    displayed: &mut usize,
    is_tty: bool,
    last_page: bool,
) -> CliResult<bool> {
    let mut stdout = std::io::stdout().lock();
    write_header(&mut stdout, columns, widths, ansi)?;

    let last_idx = cells.len().saturating_sub(1);
    for (idx, row) in cells.iter().enumerate() {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                stdout.write_all(b"  ")?;
            }
            stdout.write_all(cell.as_bytes())?;
            let vw = visible_width(cell);
            for _ in vw..widths[i] {
                stdout.write_all(b" ")?;
            }
        }
        stdout.write_all(b"\n")?;
        *displayed += 1;

        let is_end_of_results = last_page && idx == last_idx;
        if is_tty && displayed.is_multiple_of(PAGE_SIZE) && !is_end_of_results {
            drop(stdout);
            if !prompt_more()? {
                return Ok(false);
            }
            stdout = std::io::stdout().lock();
            write_header(&mut stdout, columns, widths, ansi)?;
        }
    }
    Ok(true)
}

fn write_header<W: Write>(
    w: &mut W,
    columns: &[Column],
    widths: &[usize],
    ansi: Ansi,
) -> std::io::Result<()> {
    w.write_all(ansi.bold().as_bytes())?;
    for (i, c) in columns.iter().enumerate() {
        if i > 0 {
            w.write_all(b"  ")?;
        }
        w.write_all(c.label.as_bytes())?;
        for _ in c.label.len()..widths[i] {
            w.write_all(b" ")?;
        }
    }
    w.write_all(ansi.reset().as_bytes())?;
    w.write_all(b"\n")?;
    Ok(())
}

fn visible_width(s: &str) -> usize {
    let mut width = 0usize;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c2 in chars.by_ref() {
                if c2 == 'm' {
                    break;
                }
            }
            continue;
        }
        width += 1;
    }
    width
}

fn run_json(
    _ctx: &Context,
    jmap: &Jmap,
    canonical: &str,
    filter: &Value,
    properties: Option<&[String]>,
    limit: usize,
) -> CliResult<()> {
    let mut pager = Pager::new(limit);
    let value = if let Some(props) = properties {
        let mut all: Vec<Map<String, Value>> = Vec::new();
        loop {
            let rows = fetch_rows(jmap, canonical, filter, &pager, props)?;
            if rows.is_empty() {
                break;
            }
            let last = rows.len() < limit;
            let last_id = rows
                .last()
                .and_then(|r| r.get("id").and_then(Value::as_str))
                .map(String::from);
            all.extend(rows);
            if last {
                break;
            }
            match last_id {
                Some(id) => pager.advance(id),
                None => break,
            }
        }
        Value::Array(all.into_iter().map(Value::Object).collect())
    } else {
        let mut all: Vec<String> = Vec::new();
        loop {
            let ids = fetch_ids(jmap, canonical, filter, &pager)?;
            if ids.is_empty() {
                break;
            }
            let last = ids.len() < limit;
            if let Some(id) = ids.last() {
                pager.advance(id.clone());
            }
            all.extend(ids);
            if last {
                break;
            }
        }
        Value::Array(all.into_iter().map(Value::String).collect())
    };
    writeln!(std::io::stdout(), "{}", serde_json::to_string(&value)?)?;
    Ok(())
}

fn prompt_more() -> CliResult<bool> {
    let mut err = std::io::stderr().lock();
    err.write_all(b"Show more? [Y/n] ")?;
    err.flush()?;
    drop(err);

    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(!matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "n" | "no"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_operator_detection() {
        assert!(matches!(parse_where("age>=18").unwrap().1, Op::Gte));
        assert!(matches!(parse_where("age<=18").unwrap().1, Op::Lte));
        assert!(matches!(parse_where("age>18").unwrap().1, Op::Gt));
        assert!(matches!(parse_where("age<18").unwrap().1, Op::Lt));
        assert!(matches!(parse_where("name=foo").unwrap().1, Op::Eq));
    }

    #[test]
    fn where_fails_without_op() {
        assert!(parse_where("age18").is_err());
        assert!(parse_where("=foo").is_err());
    }

    #[test]
    fn visible_width_strips_ansi() {
        assert_eq!(visible_width("\x1b[1mhi\x1b[0m"), 2);
        assert_eq!(visible_width("hello"), 5);
    }
}
