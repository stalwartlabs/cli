/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use super::{Command, Entry, EntryKind, PaletteIndex, PaletteTarget};
use crate::jmap::account::Edition;
use crate::schema::view::ViewKind;
use crate::schema::{AT_TYPE, ObjectType, Schema};
use crate::tui::route::{Route, RouteTarget};
use crate::tui::session::Connected;
use crate::tui::widgets::sidebar::LayoutTree;
use ahash::AHashMap;
use std::borrow::Cow;

const SEPARATOR: &str = " › ";
const OBJECT_PREFIX: &str = "x:";

#[derive(Clone, Copy)]
struct Place<'a> {
    layout: u8,
    section: &'a str,
    locked: bool,
}

struct Builder<'a> {
    schema: &'a Schema,
    places: AHashMap<&'a str, Place<'a>>,
    children: AHashMap<&'a str, &'a str>,
    path: String,
    index: PaletteIndex,
}

impl PaletteIndex {
    pub fn build(conn: &Connected, trees: &[LayoutTree], debug: bool) -> Self {
        let mut builder = Builder {
            schema: &conn.schema,
            places: AHashMap::new(),
            children: AHashMap::new(),
            path: String::new(),
            index: PaletteIndex::default(),
        };
        builder.links(trees);
        builder.descriptions();
        builder.forms();
        builder.commands(debug, conn.info.edition);
        builder.index
    }
}

impl<'a> Builder<'a> {
    fn links(&mut self, trees: &'a [LayoutTree]) {
        let mut crumbs: Vec<&str> = Vec::new();
        for (layout, tree) in trees.iter().enumerate() {
            let Ok(layout) = u8::try_from(layout) else {
                break;
            };
            for (node, i) in tree.links() {
                let Some(view) = node.view.as_deref() else {
                    continue;
                };
                let place = *self.places.entry(view).or_insert(Place {
                    layout,
                    section: &tree.name,
                    locked: node.locked,
                });
                if let Some(ObjectType::View { object_name }) = self.schema.objects.get(view) {
                    self.children.entry(object_name).or_insert(view);
                }
                crumbs.clear();
                crumbs.extend(tree.ancestors(i).map(|(_, n)| &*n.label));
                self.path.clear();
                self.path.push_str(&tree.name);
                for label in crumbs.iter().rev() {
                    self.path.push_str(SEPARATOR);
                    self.path.push_str(label);
                }
                let place = Place {
                    layout,
                    locked: node.locked,
                    ..place
                };
                let target = self.target(EntryKind::Link, place, view);
                self.index.push(Entry::new(
                    EntryKind::Link,
                    &node.label,
                    &self.path,
                    view,
                    "",
                    target,
                ));
            }
        }
    }

    fn descriptions(&mut self) {
        let schema = self.schema;
        for (name, kind) in &schema.objects {
            let (ObjectType::Object { description, .. }
            | ObjectType::Singleton { description, .. }) = kind
            else {
                continue;
            };
            if description.is_empty() {
                continue;
            }
            let Some((view, place)) = self.layout_view(name) else {
                continue;
            };
            self.section_path(place, view);
            let target = self.target(EntryKind::Link, place, view);
            self.index.push(
                Entry::new(EntryKind::Link, description, &self.path, view, "", target).text_label(),
            );
        }
    }

    fn forms(&mut self) {
        let schema = self.schema;
        for name in schema.objects.keys() {
            let Some(form) = schema.forms.get(name) else {
                continue;
            };
            let Some((view, place)) = self.layout_view(name) else {
                continue;
            };
            self.section_path(place, view);
            let form_target = self.target(EntryKind::Form, place, view);
            for section in &form.sections {
                if let Some(title) = section.title.as_deref().filter(|t| !t.is_empty()) {
                    self.index.push(Entry::new(
                        EntryKind::Form,
                        title,
                        &self.path,
                        view,
                        "",
                        form_target.clone(),
                    ));
                }
                for field in section.fields.iter().filter(|f| !f.label.is_empty()) {
                    let keywords = if field.name == AT_TYPE {
                        ""
                    } else {
                        field.name.as_str()
                    };
                    self.index.push(Entry::new(
                        EntryKind::Field,
                        &field.label,
                        &self.path,
                        view,
                        keywords,
                        form_target.clone(),
                    ));
                }
            }
        }
    }

    fn commands(&mut self, debug: bool, edition: Edition) {
        let commands: [(&str, Command, bool); 8] = [
            ("Account: switch", Command::Accounts, true),
            (
                "Try Enterprise",
                Command::TryEnterprise,
                edition == Edition::Community,
            ),
            ("Tools: apply plan, snapshot", Command::Tools, true),
            ("Theme: next palette", Command::Theme, true),
            ("Sidebar: always / auto-hide", Command::Sidebar, true),
            ("HTTP log", Command::HttpLog, debug),
            ("Sign out", Command::Logout, true),
            ("Quit", Command::Quit, true),
        ];
        for (label, command, enabled) in commands {
            if enabled {
                self.index.push(Entry::new(
                    EntryKind::Command,
                    label,
                    "Console",
                    "",
                    "",
                    PaletteTarget::Command(command),
                ));
            }
        }
    }

    fn layout_view(&self, name: &'a str) -> Option<(&'a str, Place<'a>)> {
        let view = match self.places.get_key_value(name) {
            Some((view, _)) => *view,
            None => *self.children.get(name)?,
        };
        Some((view, *self.places.get(view)?))
    }

    fn section_path(&mut self, place: Place<'_>, view: &str) {
        self.path.clear();
        self.path.push_str(place.section);
        self.path.push_str(SEPARATOR);
        self.path.push_str(&display_name(self.schema, view));
    }

    fn target(&self, kind: EntryKind, place: Place<'_>, view: &str) -> PaletteTarget {
        let layout = Some(place.layout);
        if place.locked {
            return PaletteTarget::Route(Route::new(layout, RouteTarget::Upsell(view.into())));
        }
        let target = match (kind, self.schema.view(view).map(|def| def.kind())) {
            (EntryKind::Link | EntryKind::Command, _) | (_, None) => {
                RouteTarget::for_view(self.schema, view)
            }
            (_, Some(ViewKind::Singleton)) => RouteTarget::Singleton(view.into()),
            (_, Some(ViewKind::CreateOnly)) => RouteTarget::Action(view.into()),
            (_, Some(ViewKind::List)) => RouteTarget::New(view.into()),
        };
        PaletteTarget::Route(Route::new(layout, target))
    }
}

fn display_name<'a>(schema: &'a Schema, view: &'a str) -> Cow<'a, str> {
    let resolved = match schema.objects.get(view) {
        Some(ObjectType::View { object_name })
            if schema
                .objects
                .get(object_name)
                .is_some_and(|parent| !matches!(parent, ObjectType::View { .. })) =>
        {
            object_name.as_str()
        }
        _ => view,
    };
    match schema.objects.get(resolved) {
        Some(ObjectType::Singleton { .. }) => {
            if let Some(title) = schema
                .forms
                .get(view)
                .or_else(|| schema.forms.get(resolved))
                .and_then(|form| form.title.as_deref())
            {
                return Cow::Borrowed(title);
            }
        }
        Some(ObjectType::Object { .. }) => {
            if let Some(singular) = schema
                .lists
                .get(view)
                .or_else(|| schema.lists.get(resolved))
                .map(|list| list.singular_name.as_str())
                && let Some(first) = singular.chars().next()
            {
                let mut name = String::with_capacity(singular.len());
                name.extend(first.to_uppercase());
                name.push_str(singular.get(first.len_utf8()..).unwrap_or_default());
                return Cow::Owned(name);
            }
        }
        _ => {}
    }
    Cow::Borrowed(view.strip_prefix(OBJECT_PREFIX).unwrap_or(view))
}
