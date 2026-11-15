/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::{ItemAction, List, MassAction};
use crate::tui::widgets::picker::PickerItem;
use serde_json::{Map, Value};
use std::collections::HashMap;

const OPEN: &str = "open";
const DELETE: &str = "delete";
const DESTRUCTIVE: &str = "cannot be undone";
const CHANGES: &str = "changes it now";
const OPENS_LIST: &str = "opens a filtered list";
const OPENS_VIEW: &str = "opens a related view";

#[derive(Debug, PartialEq, Eq)]
pub enum Chosen<'a> {
    Open,
    Delete,
    Update(&'a HashMap<String, Value>),
    Query { object: &'a str, field: &'a str },
    View(&'a str),
}

pub fn item_menu(
    list: Option<&List>,
    can_destroy: bool,
    can_update: bool,
    visible: impl Fn(&str) -> bool,
) -> Vec<PickerItem> {
    let mut items = vec![PickerItem::new(OPEN, "Open", "view or edit")];
    let mut destructive = Vec::new();
    let actions = list.map_or(&[][..], |list| list.item_actions.as_slice());
    let mut has_delete = false;
    for (index, action) in actions.iter().enumerate() {
        let (label, detail) = match action {
            ItemAction::Separator => continue,
            ItemAction::Delete { label } => {
                has_delete = true;
                if can_destroy {
                    destructive.push(PickerItem::new(
                        index.to_string(),
                        label.as_str(),
                        DESTRUCTIVE,
                    ));
                }
                continue;
            }
            ItemAction::SetProperty { label, .. } => {
                if !can_update {
                    continue;
                }
                (label.as_str(), CHANGES)
            }
            ItemAction::Query {
                label, object_name, ..
            } => {
                if !visible(object_name) {
                    continue;
                }
                (label.as_str(), OPENS_LIST)
            }
            ItemAction::View { label, object_name } => {
                if !visible(object_name) {
                    continue;
                }
                (label.as_str(), OPENS_VIEW)
            }
        };
        items.push(PickerItem::new(index.to_string(), label, detail));
    }
    if can_destroy && !has_delete {
        destructive.push(PickerItem::new(DELETE, "Delete", DESTRUCTIVE));
    }
    items.append(&mut destructive);
    items
}

pub fn mass_menu(list: Option<&List>, can_destroy: bool, can_update: bool) -> Vec<PickerItem> {
    let actions = list.map_or(&[][..], |list| list.mass_actions.as_slice());
    let mut items = Vec::with_capacity(actions.len() + 1);
    let mut destructive = Vec::new();
    let mut has_delete = false;
    for (index, action) in actions.iter().enumerate() {
        let label = match action {
            MassAction::Separator => continue,
            MassAction::Delete { label } => {
                has_delete = true;
                if can_destroy {
                    destructive.push(PickerItem::new(
                        index.to_string(),
                        label.as_str(),
                        DESTRUCTIVE,
                    ));
                }
                continue;
            }
            MassAction::SetProperty { label, .. } => {
                if !can_update {
                    continue;
                }
                label
            }
        };
        items.push(PickerItem::new(index.to_string(), label.as_str(), CHANGES));
    }
    if can_destroy && !has_delete {
        destructive.push(PickerItem::new(DELETE, "Delete", DESTRUCTIVE));
    }
    items.append(&mut destructive);
    items
}

pub fn item_choice<'a>(list: Option<&'a List>, key: &str) -> Option<Chosen<'a>> {
    match key {
        OPEN => return Some(Chosen::Open),
        DELETE => return Some(Chosen::Delete),
        _ => {}
    }
    let action = list?.item_actions.get(key.parse::<usize>().ok()?)?;
    Some(match action {
        ItemAction::Delete { .. } => Chosen::Delete,
        ItemAction::SetProperty { properties, .. } => Chosen::Update(properties),
        ItemAction::Query {
            object_name,
            field_name,
            ..
        } => Chosen::Query {
            object: object_name,
            field: field_name,
        },
        ItemAction::View { object_name, .. } => Chosen::View(object_name),
        ItemAction::Separator => return None,
    })
}

pub fn mass_choice<'a>(list: Option<&'a List>, key: &str) -> Option<Chosen<'a>> {
    if key == DELETE {
        return Some(Chosen::Delete);
    }
    match list?.mass_actions.get(key.parse::<usize>().ok()?)? {
        MassAction::Delete { .. } => Some(Chosen::Delete),
        MassAction::SetProperty { properties, .. } => Some(Chosen::Update(properties)),
        MassAction::Separator => None,
    }
}

pub fn update_args(ids: &[Box<str>], properties: &HashMap<String, Value>) -> Value {
    let patch: Map<String, Value> = properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let update: Map<String, Value> = ids
        .iter()
        .map(|id| (id.to_string(), Value::Object(patch.clone())))
        .collect();
    let mut args = Map::with_capacity(1);
    args.insert("update".into(), Value::Object(update));
    Value::Object(args)
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/browser/actions/tests.rs"]
mod tests;
