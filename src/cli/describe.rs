/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::term::Ansi;
use crate::schema::{
    EnumVariant, FieldUpdate, Fields, Filter, List, ObjectDef, ObjectSchema, ObjectType, Schema,
};
use crate::view::styled::{Color, Style, Styled};
use crate::view::type_name::{FilterKind, TypeName};
use std::collections::HashSet;
use std::fmt::Write;
use std::iter::once;

const INDENT: &str = "  ";
const DESCRIPTION_INDENT: &str = "      ";
const GAP: &str = "  ";

pub struct Describer<'a> {
    schema: &'a Schema,
    ansi: Ansi,
    out: Styled,
}

impl<'a> Describer<'a> {
    pub fn new(schema: &'a Schema, ansi: Ansi) -> Self {
        Describer {
            schema,
            ansi,
            out: Styled::default(),
        }
    }

    pub fn list_all(mut self) -> String {
        let schema = self.schema;
        let mut entries: Vec<(ObjectDef<'_>, &str)> = schema
            .objects
            .iter()
            .filter(|(name, _)| name.starts_with("x:"))
            .filter_map(|(name, kind)| match kind {
                ObjectType::Object { description, .. }
                | ObjectType::Singleton { description, .. } => {
                    Some((schema.get_object(name)?, description.as_str()))
                }
                ObjectType::View { .. } => None,
            })
            .collect();
        entries.sort_unstable_by(|a, b| a.0.display_name().cmp(b.0.display_name()));
        let width = width_of(entries.iter().map(|(def, _)| def.display_name()));
        for (def, description) in entries {
            self.column(def.display_name(), Style::Accent, width);
            self.out.plain(description);
            if def.is_singleton() {
                self.out.plain(" ");
                self.out.push("(singleton)", Style::Dim);
            }
            self.out.plain("\n");
        }
        self.finish()
    }

    pub fn object(mut self, def: ObjectDef<'_>) -> String {
        let canonical = def.name();
        let Some(
            ObjectType::Object { description, .. } | ObjectType::Singleton { description, .. },
        ) = self.schema.objects.get(canonical)
        else {
            return self.finish();
        };
        self.out.push(def.display_name(), Style::Bold);
        if def.is_singleton() {
            self.out.plain(" ");
            self.out.push("(singleton)", Style::Dim);
        }
        self.out.plain("\n");
        self.out.plain(INDENT);
        self.out.plain(description);
        self.out.plain("\n");

        match self.schema.schemas.get(canonical) {
            Some(ObjectSchema::Single { schema_name }) => {
                if let Some(fields) = self.schema.fields.get(schema_name) {
                    self.fields(None, fields);
                }
            }
            Some(ObjectSchema::Multiple { variants }) => {
                self.heading("Variants");
                let width = width_of(variants.iter().map(|v| v.name.as_str()));
                for v in variants {
                    self.out.plain(INDENT);
                    self.column(&v.name, Style::Accent, width);
                    self.out.plain(&v.label);
                    self.out.plain("\n");
                }
                for v in variants {
                    if let Some(fields) = v
                        .schema_name
                        .as_deref()
                        .and_then(|name| self.schema.fields.get(name))
                    {
                        self.fields(Some(&v.name), fields);
                    }
                }
            }
            None => {}
        }

        let filters = self.filters_of(canonical);
        if !filters.is_empty() {
            self.filters(&filters);
        }
        let mut sort = self.sort_of(canonical);
        if let Some(first) = sort.next() {
            self.heading("Sort by");
            self.out.plain(INDENT);
            self.out.push(first, Style::Accent);
            for field in sort {
                self.out.plain(", ");
                self.out.push(field, Style::Accent);
            }
            self.out.plain("\n");
        }
        self.finish()
    }

    pub fn enumeration(mut self, name: &str, variants: &[EnumVariant]) -> String {
        self.out.push(name, Style::Bold);
        self.out.plain(" ");
        self.out.push("(enum)", Style::Dim);
        self.out.plain("\n");
        self.heading("Values");
        let width = width_of(variants.iter().map(|v| v.name.as_str()));
        for v in variants {
            let style = v
                .color
                .as_deref()
                .map_or(Style::Accent, |c| Style::Badge(Color::parse(c)));
            self.out.plain(INDENT);
            self.column(&v.name, style, width);
            self.out.plain(&v.label);
            self.out.plain("\n");
            if let Some(explanation) = &v.explanation {
                self.description(explanation);
            }
        }
        self.finish()
    }

    fn fields(&mut self, variant: Option<&str>, fields: &Fields) {
        match variant {
            Some(variant) => {
                self.out.plain("\n");
                self.out.push(variant, Style::Heading);
                self.out.push(" fields", Style::Heading);
                self.out.plain("\n");
            }
            None => self.heading("Fields"),
        }
        let mut properties: Vec<_> = fields.properties.iter().collect();
        properties.sort_unstable_by_key(|(name, _)| *name);
        let width = width_of(properties.iter().map(|(name, _)| name.as_str()));
        for (name, field) in properties {
            self.out.plain(INDENT);
            self.column(name, Style::Accent, width);
            let _ = write!(self.out, "{}", TypeName(&field.typ));
            let access = match field.update {
                FieldUpdate::Mutable => None,
                FieldUpdate::Immutable => Some("create-only"),
                FieldUpdate::ServerSet => Some("read-only"),
            };
            let optional = field.typ.is_nullable().then_some("optional");
            match (optional, access) {
                (None, None) => {}
                (Some(tag), None) | (None, Some(tag)) => {
                    self.out.plain(" ");
                    self.out.push("(", Style::Dim);
                    self.out.push(tag, Style::Dim);
                    self.out.push(")", Style::Dim);
                }
                (Some(optional), Some(access)) => {
                    self.out.plain(" ");
                    self.out.push("(", Style::Dim);
                    self.out.push(optional, Style::Dim);
                    self.out.push(", ", Style::Dim);
                    self.out.push(access, Style::Dim);
                    self.out.push(")", Style::Dim);
                }
            }
            self.out.plain("\n");
            self.description(&field.description);
        }
    }

    fn filters(&mut self, filters: &[&Filter]) {
        self.heading("Filters");
        let name_width = width_of(filters.iter().map(|f| f.field()));
        let mut kind = String::new();
        let kind_width = filters
            .iter()
            .map(|f| {
                kind.clear();
                let _ = write!(kind, "{}", FilterKind(f));
                kind.chars().count()
            })
            .max()
            .unwrap_or(0);
        for filter in filters {
            kind.clear();
            let _ = write!(kind, "{}", FilterKind(filter));
            self.out.plain(INDENT);
            self.column(filter.field(), Style::Accent, name_width);
            self.column(&kind, Style::Plain, kind_width);
            self.out.push(filter.label(), Style::Dim);
            self.out.plain("\n");
        }
    }

    fn lists_of(&self, object_name: &'a str) -> impl Iterator<Item = &'a List> + use<'a> {
        let schema = self.schema;
        once(object_name)
            .chain(schema.views_of(object_name))
            .filter_map(move |key| schema.lists.get(key))
    }

    fn filters_of(&self, object_name: &'a str) -> Vec<&'a Filter> {
        let mut seen: HashSet<&str> = HashSet::new();
        self.lists_of(object_name)
            .flat_map(|list| &list.filters)
            .filter(|filter| seen.insert(filter.field()))
            .collect()
    }

    fn sort_of(&self, object_name: &'a str) -> impl Iterator<Item = &'a str> + use<'a> {
        let mut seen: HashSet<&str> = HashSet::new();
        self.lists_of(object_name)
            .flat_map(|list| &list.sort)
            .map(String::as_str)
            .filter(move |field| seen.insert(field))
    }

    fn description(&mut self, text: &str) {
        for line in text.lines() {
            self.out.plain(DESCRIPTION_INDENT);
            self.out.push(line, Style::Dim);
            self.out.plain("\n");
        }
    }

    fn heading(&mut self, title: &str) {
        self.out.plain("\n");
        self.out.push(title, Style::Heading);
        self.out.plain("\n");
    }

    fn column(&mut self, text: &str, style: Style, width: usize) {
        self.out.push(text, style);
        let _ = write!(
            self.out,
            "{:pad$}{GAP}",
            "",
            pad = width.saturating_sub(text.chars().count())
        );
    }

    fn finish(self) -> String {
        let mut out = String::new();
        self.ansi.paint(&self.out, &mut out);
        out
    }
}

fn width_of<'s>(names: impl Iterator<Item = &'s str>) -> usize {
    names.map(|name| name.chars().count()).max().unwrap_or(0)
}

pub fn unknown_describe_target(name: &str) -> String {
    let mut out = String::new();
    out.push_str("unknown object or enum: ");
    out.push_str(name);
    out.push('\n');
    out
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/cli/describe/tests.rs"]
mod tests;
