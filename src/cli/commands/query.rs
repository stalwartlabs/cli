/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::args::QueryArgs;
use crate::cli::commands::{CommandError, property};
use crate::cli::context::Context;
use crate::cli::pager::{More, PageEnd, TtyPager};
use crate::cli::term::{Ansi, write_json_line};
use crate::error::Result;
use crate::input::{Where, build_filter};
use crate::jmap::pages::{Fetch, Page};
use crate::schema::object::FieldSet;
use crate::schema::{ObjectDef, Schema};
use crate::view::{Column, LabelCache, Styled, Table, ValueFormatter};
use serde_json::{Map, Value};
use std::io::{self, Write};

const MAX_CELL_WIDTH: usize = 60;

pub fn run(ctx: &Context, args: &QueryArgs) -> Result<()> {
    let def = ctx.schema.object_no_variant(&args.object)?;
    if def.is_singleton() {
        return Err(CommandError::SingletonQuery(def.display_name().to_owned()).into());
    }

    let fields = def.all_fields();
    let clauses = args
        .wheres
        .iter()
        .map(|raw| Where::parse(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let filter = build_filter(fields, &clauses)?;
    let columns = columns(&ctx.schema, def, fields, args.fields.as_deref())?;

    if args.json {
        let properties: Option<Vec<&str>> = columns
            .as_ref()
            .map(|columns| columns.iter().map(|col| col.name).collect());
        return run_json(ctx, def, &filter, properties.as_deref());
    }
    match columns {
        None => run_ids(ctx, def, &filter),
        Some(columns) => run_table(ctx, def, fields, &filter, &columns),
    }
}

fn columns<'s>(
    schema: &'s Schema,
    def: ObjectDef<'s>,
    fields: FieldSet<'s>,
    user: Option<&[String]>,
) -> Result<Option<Vec<Column<'s>>>> {
    let Some(raws) = user else {
        return Ok(Column::list(def, fields));
    };
    raws.iter()
        .map(|raw| {
            let (name, field) = property(fields, raw)?;
            Ok(Column::named(schema, def, name, field))
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn page_end(page: &Page, limit: usize) -> PageEnd {
    if page.ids.len() < limit {
        PageEnd::Last
    } else {
        PageEnd::NotLast
    }
}

fn run_ids(ctx: &Context, def: ObjectDef<'_>, filter: &Value) -> Result<()> {
    let limit = ctx.client.max_get();
    let mut tty = TtyPager::new(ctx.paging);
    let mut out = io::stdout().lock();

    for page in ctx.client.pages(def.name(), Some(filter), Fetch::Ids) {
        let page = page?;
        let at = page_end(&page, limit);
        for id in &page.ids {
            writeln!(out, "{id}")?;
            if tty.after_row(at)? == More::Stop {
                return Ok(());
            }
        }
        if at == PageEnd::Last {
            break;
        }
        if tty.after_page()? == More::Stop {
            return Ok(());
        }
    }
    Ok(())
}

fn run_table(
    ctx: &Context,
    def: ObjectDef<'_>,
    fields: FieldSet<'_>,
    filter: &Value,
    columns: &[Column<'_>],
) -> Result<()> {
    let limit = ctx.client.max_get();
    let ansi = Ansi::new(ctx.color);
    let properties: Vec<&str> = columns.iter().map(|col| col.name).collect();
    let fetch = Fetch::Objects {
        properties: Some(&properties),
    };
    let mut tty = TtyPager::new(ctx.paging);
    let mut labels = LabelCache::default();
    let mut table = Table {
        header: columns.iter().map(|col| col.label).collect(),
        rows: Vec::new(),
    };
    let mut line = String::new();

    for page in ctx.client.pages(def.name(), Some(filter), fetch) {
        let page = page?;
        let at_end = page_end(&page, limit);
        labels.populate(&ctx.client, &ctx.schema, fields, &page.objects);

        let values = ValueFormatter::new(&ctx.schema, &labels);
        table.rows.clear();
        table
            .rows
            .extend(page.objects.iter().map(|row| cells(columns, row, &values)));
        let widths = table.widths();

        let mut out = io::stdout().lock();
        if !table.rows.is_empty() {
            write_header(&mut out, &mut line, ansi, &table, &widths)?;
        }
        let last_index = table.rows.len().saturating_sub(1);
        for (index, row) in table.rows.iter().enumerate() {
            line.clear();
            ansi.table_row(&mut line, 0, row, &widths);
            out.write_all(line.as_bytes())?;
            let at = if at_end == PageEnd::Last && index == last_index {
                PageEnd::Last
            } else {
                PageEnd::NotLast
            };
            match tty.after_row(at)? {
                More::Continue => {}
                More::Resumed => write_header(&mut out, &mut line, ansi, &table, &widths)?,
                More::Stop => return Ok(()),
            }
        }
        if at_end == PageEnd::Last {
            break;
        }
    }
    Ok(())
}

fn write_header(
    out: &mut dyn Write,
    line: &mut String,
    ansi: Ansi,
    table: &Table<'_>,
    widths: &[usize],
) -> io::Result<()> {
    line.clear();
    ansi.table_header(line, 0, &table.header, widths);
    out.write_all(line.as_bytes())
}

fn cells(
    columns: &[Column<'_>],
    row: &Map<String, Value>,
    values: &ValueFormatter<'_>,
) -> Vec<Styled> {
    Column::cells(columns, row, values)
        .map(|mut cell| {
            cell.tidy(MAX_CELL_WIDTH);
            cell
        })
        .collect()
}

fn run_json(
    ctx: &Context,
    def: ObjectDef<'_>,
    filter: &Value,
    properties: Option<&[&str]>,
) -> Result<()> {
    let mut out = io::stdout().lock();
    match properties {
        Some(properties) => {
            let fetch = Fetch::Objects {
                properties: Some(properties),
            };
            for page in ctx.client.pages(def.name(), Some(filter), fetch) {
                for row in &page?.objects {
                    write_json_line(&mut out, row)?;
                }
            }
        }
        None => {
            for page in ctx.client.pages(def.name(), Some(filter), Fetch::Ids) {
                for id in &page?.ids {
                    write_json_line(&mut out, id)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/cli/commands/query/tests.rs"]
mod tests;
