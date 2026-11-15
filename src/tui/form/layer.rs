/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Error;
use crate::tui::app::{Cx, ViewCx};
use crate::tui::exec::Slot;
use crate::tui::form::view::Pick;
use crate::tui::job::objects::{
    BulkAction, BulkSelection, ObjectsKind, ObjectsSlot, PICKER_FULL_LIMIT, PickerOptions,
    PickerSpec, Reply, Upload,
};
use crate::tui::keymap::Context;
use crate::tui::route::Route;
use crate::tui::screens::detail::doc;
use crate::tui::screens::objects_job;
use crate::tui::theme::Theme;
use crate::tui::widgets::dialog::{self, COPY, Dialog};
use crate::tui::widgets::picker::{self, Filtering, Picker, PickerItem};
use crate::view::ObjectDoc;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::text::Line;
use serde_json::Value;
use std::time::{Duration, Instant};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    Notice,
    Destroy(Box<[Box<str>]>),
    FilterChoice(usize),
    ItemMenu(Box<str>),
    MassMenu,
    Apply(Box<Value>),
    Save(Box<(Value, Vec<Upload>)>),
    FieldPick(Box<Pick>),
    Discard,
    Leave(Box<Route>),
    Reload,
    Created(Box<str>),
    Bulk(Box<(BulkSelection, BulkAction)>),
}

#[derive(Debug)]
pub enum Layer {
    Dialog {
        dialog: Dialog,
        purpose: Purpose,
    },
    Picker {
        picker: Picker,
        purpose: Purpose,
    },
    Doc {
        dialog: Dialog,
        pending: Option<Box<(Box<str>, Value)>>,
        purpose: Purpose,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum LayerEvent {
    Pending,
    Closed(Purpose),
    Confirmed(Purpose),
    Chosen {
        purpose: Purpose,
        items: Vec<(Box<str>, Box<str>)>,
    },
}

#[derive(Debug)]
struct Search {
    spec: PickerSpec,
    changed: Changed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Changed {
    No,
    Unstamped,
    At(Instant),
}

#[derive(Debug, Default)]
pub struct Layers {
    stack: Vec<Layer>,
    search: Option<Search>,
}

const DOC_NOTICE: &str =
    "Values such as generated secrets are shown only once; c copies everything.";

impl Layers {
    pub fn push(&mut self, layer: Layer) {
        self.stack.push(layer);
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    pub fn push_doc(&mut self, title: &str, object: &str, value: Value, purpose: Purpose) {
        self.stack.push(Layer::Doc {
            dialog: Dialog::info(title, Vec::new()).with_keys(COPY),
            pending: Some(Box::new((object.into(), value))),
            purpose,
        });
    }

    pub fn build_docs(&mut self, cx: &ViewCx<'_>) {
        let schema = &cx.conn.schema;
        for layer in &mut self.stack {
            let Layer::Doc {
                dialog, pending, ..
            } = layer
            else {
                continue;
            };
            let Some(pending) = pending.take() else {
                continue;
            };
            let (object, value) = &*pending;
            let mut lines = Vec::new();
            if let Some(def) = schema.get_object(object) {
                let doc = ObjectDoc::build(schema, &cx.conn.labels, def, value);
                doc::lines(&doc, cx.theme, usize::MAX, &mut lines);
            }
            if lines.is_empty() {
                lines.extend(
                    serde_json::to_string_pretty(value)
                        .unwrap_or_default()
                        .lines()
                        .map(|line| Line::raw(line.to_owned())),
                );
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(DOC_NOTICE, cx.theme.muted()));
            dialog.set_body(lines);
        }
    }

    pub fn top_picker(&mut self) -> Option<&mut Picker> {
        match self.stack.last_mut()? {
            Layer::Picker { picker, .. } => Some(picker),
            Layer::Dialog { .. } | Layer::Doc { .. } => None,
        }
    }

    pub fn context(&self) -> Option<Context> {
        self.stack.last().map(|layer| match layer {
            Layer::Dialog { dialog, .. } => dialog.context(),
            Layer::Doc { .. } => Context::Created,
            Layer::Picker { .. } => Context::Picker,
        })
    }

    pub fn handle(&mut self, key: KeyEvent) -> LayerEvent {
        let Some(top) = self.stack.last_mut() else {
            return LayerEvent::Pending;
        };
        match top {
            Layer::Dialog { dialog, .. } | Layer::Doc { dialog, .. } => match dialog.handle(key) {
                dialog::Outcome::Pending => LayerEvent::Pending,
                dialog::Outcome::Cancel => self.pop_with(LayerEvent::Closed),
                dialog::Outcome::Key(_) => self.pop_with(LayerEvent::Confirmed),
            },
            Layer::Picker { picker, .. } => {
                let outcome = picker.handle(key);
                self.picker_outcome(outcome)
            }
        }
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(picker) = self.top_picker() {
            picker.paste(text);
            self.query_changed();
        }
    }

    pub fn load_picker(&mut self, cx: &mut Cx<'_>, spec: PickerSpec) {
        if let Some(options) = cx.conn.pickers.get(&spec) {
            self.set_options(options, cx.now);
            return;
        }
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Picker));
        let kind = ObjectsKind::PickerOptions(Box::new(spec));
        cx.fx.spawn(ticket, objects_job(cx, kind));
    }

    pub fn picker_options(&mut self, cx: &mut Cx<'_>, reply: Reply<PickerOptions>) {
        match reply {
            Ok(options) => {
                self.set_options(&options, cx.now);
                cx.conn.pickers.put(*options);
            }
            Err(err) => self.options_failed(&err),
        }
    }

    pub fn tick(&mut self, cx: &mut Cx<'_>, now: Instant) {
        if let Some(spec) = self.search_due(now) {
            self.load_picker(cx, spec);
        }
    }

    fn set_options(&mut self, options: &PickerOptions, now: Instant) {
        let Some(picker) = self.top_picker().filter(|picker| picker.is_loading()) else {
            return;
        };
        let remote = options.spec.search_field.is_some()
            && (options.truncated || options.spec.search.is_some());
        if remote {
            picker.set_filtering(Filtering::Remote);
        }
        picker.set_items(
            options
                .options
                .iter()
                .map(|(id, label)| PickerItem::new(id.clone(), label.clone(), id.clone()))
                .collect(),
        );
        if options.truncated {
            let hint = if remote {
                "type to search the server"
            } else {
                "type to narrow"
            };
            picker.set_status(format!("showing the first {PICKER_FULL_LIMIT}; {hint}"));
        }
        let typed = !picker.query().is_empty();
        if remote && self.search.is_none() {
            self.search = Some(Search {
                spec: options.spec.searching(""),
                changed: if typed { Changed::At(now) } else { Changed::No },
            });
        }
    }

    fn options_failed(&mut self, err: &Error) {
        if let Some(picker) = self.top_picker().filter(|picker| picker.is_loading()) {
            picker.set_status(err.to_string());
        }
    }

    pub fn stamp(&mut self, now: Instant) {
        if let Some(search) = &mut self.search
            && search.changed == Changed::Unstamped
        {
            search.changed = Changed::At(now);
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        match self.search.as_ref()?.changed {
            Changed::At(at) => Some(at + SEARCH_DEBOUNCE),
            Changed::No | Changed::Unstamped => None,
        }
    }

    pub fn search_due(&mut self, now: Instant) -> Option<PickerSpec> {
        let search = self.search.as_mut()?;
        match search.changed {
            Changed::At(at) if now >= at + SEARCH_DEBOUNCE => {}
            Changed::No | Changed::Unstamped | Changed::At(_) => return None,
        }
        search.changed = Changed::No;
        let picker = match self.stack.last_mut() {
            Some(Layer::Picker { picker, .. }) => picker,
            Some(Layer::Dialog { .. } | Layer::Doc { .. }) | None => return None,
        };
        picker.reload("searching…");
        Some(search.spec.searching(picker.query()))
    }

    fn query_changed(&mut self) {
        if let Some(search) = &mut self.search {
            search.changed = Changed::Unstamped;
        }
    }

    fn picker_outcome(&mut self, outcome: picker::Outcome) -> LayerEvent {
        let indexes = match outcome {
            picker::Outcome::Pending => return LayerEvent::Pending,
            picker::Outcome::QueryChanged => {
                self.query_changed();
                return LayerEvent::Pending;
            }
            picker::Outcome::Cancel => return self.pop_with(LayerEvent::Closed),
            picker::Outcome::Chosen(index) => Ok(index),
            picker::Outcome::ChosenMany(items) => Err(items),
        };
        self.search = None;
        let Some(Layer::Picker { picker, purpose }) = self.stack.pop() else {
            return LayerEvent::Pending;
        };
        let items = match indexes {
            Ok(index) => picker
                .item(index)
                .map(|item| (item.key.clone(), item.label.clone()))
                .into_iter()
                .collect(),
            Err(items) => items,
        };
        LayerEvent::Chosen { purpose, items }
    }

    fn pop_with(&mut self, event: fn(Purpose) -> LayerEvent) -> LayerEvent {
        if matches!(self.stack.last(), Some(Layer::Picker { .. })) {
            self.search = None;
        }
        match self.stack.pop() {
            Some(
                Layer::Dialog { purpose, .. }
                | Layer::Picker { purpose, .. }
                | Layer::Doc { purpose, .. },
            ) => event(purpose),
            None => LayerEvent::Pending,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        for layer in &mut self.stack {
            match layer {
                Layer::Dialog { dialog, .. } | Layer::Doc { dialog, .. } => {
                    dialog.render(frame, area, theme)
                }
                Layer::Picker { picker, .. } => picker.render(frame, area, theme),
            }
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/form/layer/tests.rs"]
mod tests;
