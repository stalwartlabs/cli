/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod actions;
pub mod filters;
pub mod paging;
pub mod rows;

use crate::jmap::account::Op;
use crate::jmap::query::PageResult;
use crate::jmap::response::SetResponse;
use crate::plan::bulk::{BulkEvent, BulkStatus, BulkSummary};
use crate::schema::object::FieldSet;
use crate::schema::view::ViewDef;
use crate::schema::{AT_TYPE, Filter};
use crate::tui::app::{Cx, ScreenMsg, ViewCx};
use crate::tui::exec::{Cancel, Slot};
use crate::tui::form::layer::{Layer, LayerEvent, Layers, Purpose};
use crate::tui::form::model::{allowed, referenced_objects};
use crate::tui::job::objects::{
    BulkAction, BulkSelection, BulkSpec, ObjectsKind, ObjectsOutput, ObjectsSlot, PageSpec,
    PickerSpec, SetSpec,
};
use crate::tui::keymap::{self, Context, code, ctrl, key, keys, upper};
use crate::tui::msg::{JobOutput, JobProgress};
use crate::tui::route::{ObjectRef, Route, RouteTarget};
use crate::tui::screens::objects_job;
use crate::tui::session::{Access, Connected};
use crate::tui::theme::Level;
use crate::tui::widgets::chrome::Crumbs;
use crate::tui::widgets::dialog::{CONFIRM_APPLY, CONFIRM_DESTROY, CONFIRM_LEAVE, Dialog, Risk};
use crate::tui::widgets::picker::{Picker, Selection};
use crate::tui::widgets::table::{Cursor, TableColumn, TableView};
use crate::view::labels::Labels;
use crate::view::set_error_plain;
use crate::view::{LabelCache, RefLabels};
use actions::{Chosen, update_args};
use filters::{FilterEvent, Filters, enum_items};
use paging::{Arrival, PageRequest, Pager};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use rows::RowBuilder;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Write;
use std::time::Instant;

const DEFAULT_HEIGHT: usize = 30;
const MAX_DESTROY_PREVIEW: usize = 8;
const MAX_FAILURES_SHOWN: usize = 20;
const REQUEST_LABEL: &str = "show request";
const DELETE_VERB: &str = "Delete";
pub const READ_ONLY: &str = "This list is read-only for this account";
pub const RUNNING: &str = "A mass action is running; esc cancels it";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Open,
    New,
    Delete,
    Mark,
    MarkPage,
    MarkAll,
    Sort,
    Menu,
    Filter,
    Reload,
    Back,
}

keys! { pub KEYS, pub HINTS: Action;
    "j/k" Down "move" true [key('j'), code(KeyCode::Down)],
    "k" Up "up" false [key('k'), code(KeyCode::Up)],
    "pgdn" PageDown "page down" false [code(KeyCode::PageDown), ctrl('f')],
    "pgup" PageUp "page up" false [code(KeyCode::PageUp), ctrl('b')],
    "g/G" Home "first / last" false [key('g'), code(KeyCode::Home)],
    "G" End "last" false [upper('G'), code(KeyCode::End)],
    "⏎" Open "open" true [code(KeyCode::Enter)],
    "/" Filter "filter" true [key('/')],
    "x" Menu "actions" true [key('x')],
    "n" New "new" true [key('n')],
    "d" Delete "delete" true [key('d'), code(KeyCode::Delete)],
    "space" Mark "mark" true [code(KeyCode::Char(' '))],
    "*" MarkAll "mark all matching" true [key('*')],
    "v" MarkPage "mark page" false [key('v')],
    "o" Sort "sort" true [key('o')],
    "R" Reload "reload" false [upper('R')],
    "esc" Back "back" false [code(KeyCode::Esc), code(KeyCode::Backspace)],
}

pub struct State {
    layout: Option<u8>,
    view: Box<str>,
    object: Box<str>,
    title: Box<str>,
    plural: Box<str>,
    singular: Box<str>,
    columns: Vec<TableColumn>,
    rows: RowBuilder,
    filter: Option<Value>,
    filters: Filters,
    preset: Option<(Box<str>, Box<str>)>,
    preset_crumb: Option<Box<str>>,
    pager: Pager,
    cursor: Cursor,
    height: usize,
    forward: bool,
    marks: HashSet<Box<str>>,
    sorts: Box<[Box<str>]>,
    sort: Option<(usize, bool)>,
    layers: Layers,
    error: Option<Box<str>>,
    awaiting: Vec<(i64, PageResult)>,
    can_create: bool,
    can_destroy: bool,
    can_update: bool,
    all_matching: bool,
    running: Option<Running>,
    follow: Option<Box<str>>,
}

struct Running {
    cancel: Cancel,
    progress: Option<BulkEvent>,
}

impl State {
    pub fn new(
        conn: &Connected,
        layout: Option<u8>,
        view: &str,
        preset: Option<(&str, &str)>,
    ) -> Self {
        let schema = &conn.schema;
        let def = schema.view(view);
        let list = def.as_ref().and_then(|def| def.list);
        let object: Box<str> = def.as_ref().map_or(view, |def| def.object.name()).into();
        let title = list.map_or(view, |list| list.title.as_str());
        let plural = list.map_or(view, |list| list.plural_name.as_str());
        let singular = list.map_or("object", |list| list.singular_name.as_str());
        let permissions = &conn.info.permissions;
        let filters = Filters::new(def.as_ref(), |filter| match filter {
            Filter::ObjectId { object_name, .. } => {
                allowed(schema, permissions, object_name, Op::Get)
            }
            _ => true,
        });
        let preset_crumb =
            preset.map(|(field, value)| preset_crumb(&conn.labels, def.as_ref(), field, value));
        let preset = preset.map(|(field, value)| (Box::from(field), Box::from(value)));
        let filter = def
            .as_ref()
            .and_then(|def| filters.build(def).ok())
            .flatten();
        let filter = with_preset(filter, preset.as_ref());
        let rows = RowBuilder::new(
            list.map(|list| list.columns.as_slice()).unwrap_or_default(),
            def.as_ref().and_then(|def| def.label_property()),
            def.as_ref()
                .filter(|def| def.object.is_multi_variant())
                .map(|_| AT_TYPE),
        );
        let columns = rows.labels().map(TableColumn::new).collect();
        State {
            layout,
            view: view.into(),
            object,
            title: title.into(),
            plural: plural.into(),
            singular: singular.into(),
            columns,
            rows,
            filter,
            filters,
            preset,
            preset_crumb,
            pager: Pager::new(conn.client.max_get()),
            cursor: Cursor::default(),
            height: DEFAULT_HEIGHT,
            forward: true,
            marks: HashSet::new(),
            sorts: list
                .map(|list| list.sort.iter().map(|name| name.as_str().into()).collect())
                .unwrap_or_default(),
            sort: None,
            layers: Layers::default(),
            error: None,
            awaiting: Vec::new(),
            can_create: allowed(schema, &conn.info.permissions, view, Op::Create),
            can_destroy: allowed(schema, &conn.info.permissions, view, Op::Destroy),
            can_update: allowed(schema, &conn.info.permissions, view, Op::Update),
            all_matching: false,
            running: None,
            follow: None,
        }
    }

    pub fn start(&mut self, cx: &mut Cx<'_>) {
        self.fetch(cx);
    }

    pub fn resume(&mut self, cx: &mut Cx<'_>) {
        self.follow = self.current_id().map(Box::from);
        self.reload(cx);
    }

    fn total(&self) -> usize {
        self.pager.total().unwrap_or(0)
    }

    fn current_id(&self) -> Option<&str> {
        self.pager.row(self.cursor.selected).map(|row| &*row.id)
    }

    fn fetch(&mut self, cx: &mut Cx<'_>) {
        let viewport = self.cursor.visible(self.height);
        while let Some(request) = self.pager.request(viewport.clone(), self.forward) {
            self.spawn_page(cx, request);
        }
    }

    fn spawn_page(&self, cx: &mut Cx<'_>, request: PageRequest) {
        let page = u32::try_from(request.page).unwrap_or(u32::MAX);
        let spec = PageSpec {
            object: self.object.clone(),
            filter: self.filter.clone(),
            sort: self
                .sort
                .and_then(|(index, ascending)| Some((self.sorts.get(index)?.clone(), ascending))),
            position: i64::try_from(request.position).unwrap_or(i64::MAX),
            limit: request.limit,
            total: request.total,
            properties: self.rows.properties().into(),
        };
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Page(page)));
        cx.fx
            .spawn(ticket, objects_job(cx, ObjectsKind::Page(Box::new(spec))));
    }

    fn reload(&mut self, cx: &mut Cx<'_>) {
        cx.slots.bump_where(|slot| {
            matches!(
                slot,
                Slot::Objects(ObjectsSlot::Page(_) | ObjectsSlot::Labels(_))
            )
        });
        self.awaiting.clear();
        self.pager.reset();
        self.error = None;
        self.fetch(cx);
    }

    fn arrive(&mut self, cx: &mut Cx<'_>, position: i64, result: PageResult) {
        let fields = cx
            .conn
            .schema
            .get_object(&self.object)
            .map_or(FieldSet::Empty, |def| def.all_fields());
        let request = cx
            .conn
            .labels
            .missing(&cx.conn.schema, fields, &result.objects);
        if request.is_empty() {
            self.store(cx, position, &result);
            return;
        }
        let page = u32::try_from(position).unwrap_or(u32::MAX);
        self.awaiting.push((position, result));
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Labels(page)));
        let kind = ObjectsKind::Labels {
            position,
            request: Box::new(request),
        };
        cx.fx.spawn(ticket, objects_job(cx, kind));
    }

    fn labelled(&mut self, cx: &mut Cx<'_>, position: i64, labels: Labels) {
        cx.conn.labels.insert_all(labels);
        if let Some(index) = self.awaiting.iter().position(|(p, _)| *p == position) {
            let (_, result) = self.awaiting.swap_remove(index);
            self.store(cx, position, &result);
        }
    }

    fn store(&mut self, cx: &mut Cx<'_>, position: i64, result: &PageResult) {
        let page = usize::try_from(position).unwrap_or(0) / self.pager.page_size();
        let total = result.total.and_then(|t| usize::try_from(t).ok());
        let rows = self
            .rows
            .build(&cx.conn.schema, &cx.conn.labels, &self.object, result);
        for row in &rows {
            for (column, cell) in self.columns.iter_mut().zip(row.cells.iter()) {
                column.fit(cell);
            }
        }
        let page_size = self.pager.page_size();
        let followed = self.follow.take().and_then(|id| {
            rows.iter()
                .position(|row| row.id == id)
                .map(|index| page * page_size + index)
        });
        if self.pager.arrive(page, rows, total, self.cursor.selected) == Arrival::PastEnd {
            self.cursor = Cursor::default();
        }
        let total = self.total();
        if let Some(position) = followed {
            self.cursor.move_to(position, total);
            self.cursor.scroll_into_view(self.height);
        }
        self.cursor.move_to(self.cursor.selected, total);
        self.fetch(cx);
    }

    fn move_by(&mut self, cx: &mut Cx<'_>, delta: isize) {
        self.forward = delta >= 0;
        self.cursor.move_by(delta, self.total());
        self.cursor.scroll_into_view(self.height);
        self.fetch(cx);
    }

    fn cycle_sort(&mut self, cx: &mut Cx<'_>) {
        if self.sorts.is_empty() {
            cx.flash
                .set(Level::Info, "This list has no sortable columns", cx.now);
            return;
        }
        self.sort = match self.sort {
            None => Some((0, true)),
            Some((index, true)) => Some((index, false)),
            Some((index, false)) if index + 1 < self.sorts.len() => Some((index + 1, true)),
            Some((_, false)) => None,
        };
        self.cursor = Cursor::default();
        self.reload(cx);
    }

    fn sort_label(&self) -> Option<(&str, bool)> {
        let (index, ascending) = self.sort?;
        let name = self.sorts.get(index)?;
        let label = self
            .rows
            .names()
            .zip(self.rows.labels())
            .find(|(column, _)| *column == &**name)
            .map_or(&**name, |(_, label)| label);
        Some((label, ascending))
    }

    fn mark_page(&mut self) {
        let visible = self.cursor.visible(self.height);
        let pager = &self.pager;
        let all = visible
            .clone()
            .filter_map(|position| pager.row(position))
            .all(|row| self.marks.contains(&row.id));
        for row in visible.filter_map(|position| pager.row(position)) {
            if all {
                self.marks.remove(&row.id);
            } else if !self.marks.contains(&row.id) {
                self.marks.insert(row.id.clone());
            }
        }
        self.all_matching = false;
    }

    fn targets(&self) -> Option<Box<[Box<str>]>> {
        if self.marks.is_empty() {
            self.current_id().map(|id| Box::from([Box::from(id)]))
        } else {
            Some(self.marks.iter().cloned().collect())
        }
    }

    fn question(&self, verb: &str, count: usize) -> String {
        let mut text = String::with_capacity(verb.len() + 32);
        let _ = write!(text, "{verb}: {count} {}", self.noun(count));
        if self.all_matching {
            text.push_str(" (all matching the current filter)");
        }
        text.push('?');
        text
    }

    fn push_names<'i>(&self, body: &mut Vec<Line<'static>>, ids: impl Iterator<Item = &'i str>) {
        let mut shown = 0;
        let mut more = 0;
        for id in ids {
            if shown == MAX_DESTROY_PREVIEW {
                more += 1;
                continue;
            }
            shown += 1;
            let label = self
                .pager
                .rows()
                .find(|row| &*row.id == id)
                .map_or(id, |row| &*row.label);
            body.push(Line::from(vec![
                Span::raw("  • "),
                Span::raw(label.to_string()),
            ]));
        }
        if more > 0 {
            body.push(Line::raw(format!("  … and {more} more")));
        }
    }

    fn request_lines(&self, args: &Value) -> Vec<Line<'static>> {
        vec![
            Line::raw("Request sent to the server:"),
            Line::raw(""),
            Line::raw(format!("{}/set {}", self.object, args)),
        ]
    }

    fn confirm_delete(&mut self, ids: Box<[Box<str>]>, verb: &str) {
        let mut body = Vec::with_capacity(ids.len().min(MAX_DESTROY_PREVIEW) + 4);
        body.push(Line::raw(self.question(verb, ids.len())));
        body.push(Line::raw(""));
        self.push_names(&mut body, ids.iter().map(|id| &**id));
        body.push(Line::raw(""));
        body.push(Line::raw("This cannot be undone."));
        let raw = self.request_lines(&json!({ "destroy": &*ids }));
        self.layers.push(Layer::Dialog {
            dialog: Dialog::confirm(verb, Risk::High, body, CONFIRM_DESTROY)
                .with_alternate(raw, REQUEST_LABEL),
            purpose: Purpose::Destroy(ids),
        });
    }

    fn confirm_update(
        &mut self,
        ids: &[Box<str>],
        properties: &HashMap<String, Value>,
        verb: &str,
    ) {
        let args = update_args(ids, properties);
        let mut body = Vec::with_capacity(ids.len().min(MAX_DESTROY_PREVIEW) + 2);
        body.push(Line::raw(self.question(verb, ids.len())));
        body.push(Line::raw(""));
        self.push_names(&mut body, ids.iter().map(|id| &**id));
        let raw = self.request_lines(&args);
        self.layers.push(Layer::Dialog {
            dialog: Dialog::confirm(verb, Risk::Low, body, CONFIRM_APPLY)
                .with_alternate(raw, REQUEST_LABEL),
            purpose: Purpose::Apply(Box::new(args)),
        });
    }

    fn noun(&self, count: usize) -> &str {
        if count == 1 {
            &self.singular
        } else {
            &self.plural
        }
    }

    fn send_set(&mut self, cx: &mut Cx<'_>, args: Value) {
        let spec = SetSpec {
            object: self.object.clone(),
            args,
            uploads: Vec::new(),
        };
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Save));
        cx.fx
            .spawn(ticket, objects_job(cx, ObjectsKind::Set(Box::new(spec))));
    }

    fn set_done(&mut self, cx: &mut Cx<'_>, response: &SetResponse) {
        cx.conn.pickers.forget(&self.object);
        let mut text = String::new();
        let failure = response
            .not_destroyed
            .first()
            .or_else(|| response.not_updated.first());
        match failure {
            Some((id, err)) => {
                let label = self
                    .pager
                    .rows()
                    .find(|row| &*row.id == id)
                    .map_or(id, |row| &*row.label);
                let title = if response.not_destroyed.first().is_none() {
                    "Could not update"
                } else {
                    "Could not delete"
                };
                let mut body = vec![Line::raw(label.to_string()), Line::raw("")];
                body.extend(
                    set_error_plain(err, &RefLabels::default())
                        .lines()
                        .map(|line| Line::raw(line.to_string())),
                );
                self.layers.push(Layer::Dialog {
                    dialog: Dialog::info(title, body),
                    purpose: Purpose::Notice,
                });
            }
            None => {
                match (response.destroyed.len(), response.updated.len()) {
                    (0, updated) => {
                        let _ = write!(text, "Updated {updated}");
                    }
                    (destroyed, _) => {
                        let _ = write!(text, "Deleted {destroyed}");
                    }
                }
                cx.flash.set(Level::Ok, text, cx.now);
            }
        }
        for id in &response.destroyed {
            self.marks.remove(id.as_str());
        }
        self.follow = match response.updated.keys().next() {
            Some(id) if response.updated.len() == 1 => Some(id.as_str().into()),
            _ => None,
        };
        self.reload(cx);
    }

    fn open(&self, cx: &mut Cx<'_>, target: RouteTarget) {
        cx.router.open(Route::new(self.layout, target));
    }

    fn open_linked(&self, cx: &mut Cx<'_>, view: &str, target: RouteTarget) {
        let target = match cx.conn.access(view) {
            Access::Locked => RouteTarget::Upsell(view.into()),
            Access::Open | Access::Hidden => target,
        };
        self.open(cx, target)
    }

    fn layer_event(&mut self, cx: &mut Cx<'_>, event: LayerEvent) {
        match event {
            LayerEvent::Chosen {
                purpose: Purpose::FilterChoice(index),
                items,
            } => {
                self.filters.chosen(index, items);
                self.apply_filters(cx);
            }
            LayerEvent::Confirmed(Purpose::Destroy(ids)) => {
                self.send_set(cx, json!({ "destroy": ids }))
            }
            LayerEvent::Confirmed(Purpose::Apply(args)) => self.send_set(cx, *args),
            LayerEvent::Confirmed(Purpose::Leave(route)) => {
                self.cancel_running();
                cx.router.jump(*route);
            }
            LayerEvent::Confirmed(Purpose::Reload) => {
                self.cancel_running();
                cx.router.reload();
            }
            LayerEvent::Confirmed(Purpose::Bulk(request)) => {
                let (selection, action) = *request;
                self.run_bulk(cx, selection, action)
            }
            LayerEvent::Chosen {
                purpose: Purpose::ItemMenu(id),
                items,
            } => {
                if let Some((key, label)) = items.first() {
                    self.item_chosen(cx, id, key, label);
                }
            }
            LayerEvent::Chosen {
                purpose: Purpose::MassMenu,
                items,
            } => {
                if let Some((key, label)) = items.first() {
                    self.mass_chosen(cx, key, label);
                }
            }
            LayerEvent::Confirmed(_)
            | LayerEvent::Pending
            | LayerEvent::Closed(_)
            | LayerEvent::Chosen { .. } => {}
        }
    }
}

impl State {
    fn apply_filters(&mut self, cx: &mut Cx<'_>) {
        let Some(view) = cx.conn.schema.view(&self.view) else {
            return;
        };
        match self.filters.build(&view) {
            Ok(filter) => {
                self.filter = with_preset(filter, self.preset.as_ref());
                self.cursor = Cursor::default();
                self.marks.clear();
                self.all_matching = false;
                self.reload(cx);
            }
            Err(err) => cx.flash.set(Level::Error, err.to_string(), cx.now),
        }
    }

    fn choose_filter(&mut self, cx: &mut Cx<'_>, index: usize) {
        let schema = cx.conn.schema.clone();
        let Some(filter) = schema
            .view(&self.view)
            .and_then(|view| view.list)
            .and_then(|list| list.filters.get(index))
        else {
            return;
        };
        let mut load = None;
        let picker = match filter {
            Filter::Enum {
                enum_name, label, ..
            } => Picker::new(
                label.as_str(),
                enum_items(&schema, enum_name),
                Selection::Multi,
            ),
            Filter::ObjectId {
                object_name, label, ..
            } => {
                load = Some(PickerSpec::new(&schema, object_name, None));
                Picker::loading(label.as_str(), Selection::Multi)
            }
            Filter::Text { .. } | Filter::Integer { .. } | Filter::Date { .. } => return,
        };
        let picker = picker.with_marked(self.filters.current(index));
        self.layers.push(Layer::Picker {
            picker,
            purpose: Purpose::FilterChoice(index),
        });
        if let Some(spec) = load {
            self.layers.load_picker(cx, spec);
        }
    }

    fn filter_key(&mut self, cx: &mut Cx<'_>, event: KeyEvent) {
        let Some(action) = keymap::resolve(filters::KEYS, event) else {
            if keymap::is_text(event) {
                self.filters.text(event);
            }
            return;
        };
        let schema = cx.conn.schema.clone();
        let list = schema
            .view(&self.view)
            .and_then(|view| view.list)
            .map_or(&[][..], |list| list.filters.as_slice());
        match self.filters.act(action, list) {
            FilterEvent::Pending | FilterEvent::Close => {}
            FilterEvent::Apply => self.apply_filters(cx),
            FilterEvent::Choose(index) => self.choose_filter(cx, index),
        }
    }
}

impl State {
    fn menu(&mut self, cx: &Cx<'_>) {
        let schema = &cx.conn.schema;
        let list = schema.view(&self.view).and_then(|view| view.list);
        if !self.has_selection() {
            let Some(id) = self.current_id() else {
                return;
            };
            let id: Box<str> = id.into();
            let items = actions::item_menu(list, self.can_destroy, self.can_update, |view| {
                cx.conn.access(view) != Access::Hidden
            });
            self.layers.push(Layer::Picker {
                picker: Picker::new("Actions", items, Selection::Single),
                purpose: Purpose::ItemMenu(id),
            });
        } else {
            let items = actions::mass_menu(list, self.can_destroy, self.can_update);
            self.layers.push(Layer::Picker {
                picker: Picker::new("Marked", items, Selection::Single),
                purpose: Purpose::MassMenu,
            });
        }
    }

    fn item_chosen(&mut self, cx: &mut Cx<'_>, id: Box<str>, key: &str, label: &str) {
        let schema = cx.conn.schema.clone();
        let list = schema.view(&self.view).and_then(|view| view.list);
        match actions::item_choice(list, key) {
            Some(Chosen::Open) => self.open(
                cx,
                RouteTarget::Object {
                    view: self.view.clone(),
                    id: ObjectRef::Id(id),
                },
            ),
            Some(Chosen::Delete) => self.confirm_delete(Box::from([id]), label),
            Some(Chosen::Update(properties)) => self.confirm_update(&[id], properties, label),
            Some(Chosen::Query { object, field }) => {
                if let Some(row) = self
                    .pager
                    .rows()
                    .find(|row| row.id == id && !row.label.is_empty())
                {
                    cx.conn.labels.insert(&self.object, &id, &row.label);
                }
                self.open_linked(
                    cx,
                    object,
                    RouteTarget::Query {
                        view: object.into(),
                        field: field.into(),
                        value: id,
                    },
                )
            }
            Some(Chosen::View(view)) => self.open_linked(
                cx,
                view,
                RouteTarget::Object {
                    view: view.into(),
                    id: ObjectRef::Id(id),
                },
            ),
            None => {}
        }
    }

    fn mass_chosen(&mut self, cx: &Cx<'_>, key: &str, label: &str) {
        let schema = cx.conn.schema.clone();
        let list = schema.view(&self.view).and_then(|view| view.list);
        match actions::mass_choice(list, key) {
            Some(Chosen::Delete) => self.confirm_bulk(BulkAction::Destroy, label),
            Some(Chosen::Update(properties)) => {
                let patch = properties
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                self.confirm_bulk(BulkAction::SetProperty(patch), label)
            }
            _ => {}
        }
    }

    fn has_selection(&self) -> bool {
        self.all_matching || !self.marks.is_empty()
    }

    fn selection(&self) -> (BulkSelection, usize) {
        if self.all_matching {
            (BulkSelection::Matching(self.filter.clone()), self.total())
        } else {
            let ids: Vec<String> = self.marks.iter().map(|id| id.to_string()).collect();
            let count = ids.len();
            (BulkSelection::Ids(ids), count)
        }
    }

    fn confirm_bulk(&mut self, action: BulkAction, verb: &str) {
        let (selection, count) = self.selection();
        let (risk, keys) = match &action {
            BulkAction::Destroy => (Risk::High, CONFIRM_DESTROY),
            BulkAction::SetProperty(_) => (Risk::Low, CONFIRM_APPLY),
        };
        let mut body = vec![Line::raw(self.question(verb, count)), Line::raw("")];
        if !self.all_matching {
            self.push_names(&mut body, self.marks.iter().map(|id| &**id));
            body.push(Line::raw(""));
        }
        if matches!(action, BulkAction::Destroy) {
            body.push(Line::raw(
                "This cannot be undone. esc cancels between batches.",
            ));
        } else {
            body.push(Line::raw("esc cancels between batches."));
        }
        let mut raw = vec![Line::raw(
            "Runs in batches of the server's maxObjectsInSet.",
        )];
        match &action {
            BulkAction::Destroy => raw.push(Line::raw(format!("{}/set destroy", self.object))),
            BulkAction::SetProperty(patch) => raw.push(Line::raw(format!(
                "{}/set update, patch: {}",
                self.object,
                Value::Object(patch.clone())
            ))),
        }
        self.layers.push(Layer::Dialog {
            dialog: Dialog::confirm(verb, risk, body, keys).with_alternate(raw, REQUEST_LABEL),
            purpose: Purpose::Bulk(Box::new((selection, action))),
        });
    }

    fn confirm_cancel(&mut self, purpose: Purpose) {
        let body = vec![Line::raw(
            "A mass action is running. Leave and cancel it after the current batch?",
        )];
        self.layers.push(Layer::Dialog {
            dialog: Dialog::confirm("Mass action running", Risk::High, body, CONFIRM_LEAVE),
            purpose,
        });
    }

    fn cancel_running(&mut self) {
        if let Some(running) = self.running.take() {
            running.cancel.cancel();
        }
    }

    fn run_bulk(&mut self, cx: &mut Cx<'_>, selection: BulkSelection, action: BulkAction) {
        let cancel = Cancel::default();
        let spec = BulkSpec {
            object: self.object.clone(),
            selection,
            action,
            cancel: cancel.clone(),
        };
        self.cancel_running();
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Save));
        cx.fx
            .spawn(ticket, objects_job(cx, ObjectsKind::Bulk(Box::new(spec))));
        self.running = Some(Running {
            cancel,
            progress: None,
        });
    }

    fn bulk_done(&mut self, cx: &mut Cx<'_>, summary: &BulkSummary) {
        cx.conn.pickers.forget(&self.object);
        self.running = None;
        self.marks.clear();
        self.all_matching = false;
        if summary.failed == 0 && matches!(summary.status, BulkStatus::Complete) {
            cx.flash
                .set(Level::Ok, format!("Done: {} objects", summary.done), cx.now);
        } else {
            self.layers.push(Layer::Dialog {
                dialog: Dialog::info("Mass action summary", bulk_lines(summary)),
                purpose: Purpose::Notice,
            });
        }
        self.reload(cx);
    }
}

fn bulk_lines(summary: &BulkSummary) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(MAX_FAILURES_SHOWN + 4);
    let status = match &summary.status {
        BulkStatus::Complete => "Finished",
        BulkStatus::Cancelled => "Cancelled",
        BulkStatus::Failed(_) => "Stopped",
    };
    lines.push(Line::raw(format!(
        "{status}: {} of {} done, {} failed.",
        summary.done, summary.total, summary.failed
    )));
    if let BulkStatus::Failed(err) = &summary.status {
        lines.push(Line::raw(format!("Error: {err}")));
    }
    if !summary.failures.is_empty() {
        lines.push(Line::raw(""));
    }
    let labels = RefLabels::default();
    for (id, err) in summary.failures.iter().take(MAX_FAILURES_SHOWN) {
        lines.push(Line::raw(format!(
            "  {id}: {}",
            set_error_plain(err, &labels)
        )));
    }
    if summary.failed > MAX_FAILURES_SHOWN {
        lines.push(Line::raw(format!(
            "  … and {} more",
            summary.failed - MAX_FAILURES_SHOWN
        )));
    }
    lines
}

fn with_preset(filter: Option<Value>, preset: Option<&(Box<str>, Box<str>)>) -> Option<Value> {
    let Some((field, value)) = preset else {
        return filter;
    };
    let mut map = match filter {
        Some(Value::Object(map)) => map,
        _ => Map::with_capacity(1),
    };
    map.insert(field.to_string(), Value::String(value.to_string()));
    Some(Value::Object(map))
}

pub fn available(state: &State, action: Action) -> bool {
    let rows = state.total() > 0;
    match action {
        Action::New => state.can_create,
        Action::Delete => state.can_destroy && rows,
        Action::Sort => !state.sorts.is_empty(),
        Action::Mark | Action::MarkPage | Action::MarkAll => {
            rows && (state.can_destroy || state.can_update)
        }
        Action::Open | Action::Menu => rows,
        Action::Filter => !state.filters.is_empty(),
        Action::Up
        | Action::Down
        | Action::PageUp
        | Action::PageDown
        | Action::Home
        | Action::End
        | Action::Reload
        | Action::Back => true,
    }
}

pub fn deadline(state: &State) -> Option<Instant> {
    state.layers.deadline()
}

pub fn guard_leave(state: &mut State, route: Route) -> Option<Route> {
    if state.running.is_none() {
        return Some(route);
    }
    state.confirm_cancel(Purpose::Leave(Box::new(route)));
    None
}

pub fn guard_reload(state: &mut State) -> bool {
    if state.running.is_none() {
        return true;
    }
    state.confirm_cancel(Purpose::Reload);
    false
}

pub fn cancel_running(state: &mut State) {
    state.cancel_running();
}

pub fn is_running(state: &State) -> bool {
    state.running.is_some()
}

pub fn context(state: &State) -> Context {
    match state.layers.context() {
        Some(ctx) => ctx,
        None if state.filters.is_editing() => Context::Filter,
        None => Context::Browser,
    }
}

pub fn handle_key(state: &mut State, cx: &mut Cx<'_>, event: KeyEvent) -> bool {
    if !state.layers.is_empty() {
        let event = state.layers.handle(event);
        state.layers.stamp(cx.now);
        state.layer_event(cx, event);
        return true;
    }
    if state.filters.is_editing() {
        state.filter_key(cx, event);
        return true;
    }
    match keymap::resolve(KEYS, event) {
        Some(action) => {
            update(state, cx, ScreenMsg::Key(action));
            true
        }
        None => false,
    }
}

pub fn update(state: &mut State, cx: &mut Cx<'_>, msg: ScreenMsg<'_, Action>) {
    match msg {
        ScreenMsg::Key(action) => act(state, cx, action),
        ScreenMsg::Paste(text) if state.layers.is_empty() => state.filters.paste(text),
        ScreenMsg::Paste(text) => {
            state.layers.paste(text);
            state.layers.stamp(cx.now);
        }
        ScreenMsg::Tick(now) => state.layers.tick(cx, now),
        ScreenMsg::Done(JobOutput::Objects(output)) => done(state, cx, output),
        ScreenMsg::Progress(JobProgress::Bulk(event)) => {
            if let Some(running) = &mut state.running {
                running.progress = Some(event);
            }
        }
        ScreenMsg::Text(_)
        | ScreenMsg::Done(_)
        | ScreenMsg::Stream(_)
        | ScreenMsg::Resumed(_)
        | ScreenMsg::Progress(_) => {}
    }
}

fn act(state: &mut State, cx: &mut Cx<'_>, action: Action) {
    if state.running.is_some()
        && !matches!(
            action,
            Action::Up
                | Action::Down
                | Action::PageUp
                | Action::PageDown
                | Action::Home
                | Action::End
                | Action::Reload
                | Action::Back
        )
    {
        cx.flash.set(Level::Warn, RUNNING, cx.now);
        return;
    }
    let page = isize::try_from(state.height.max(1)).unwrap_or(1);
    match action {
        Action::Up => state.move_by(cx, -1),
        Action::Down => state.move_by(cx, 1),
        Action::PageUp => state.move_by(cx, -page),
        Action::PageDown => state.move_by(cx, page),
        Action::Home => state.move_by(cx, isize::MIN),
        Action::End => state.move_by(cx, isize::MAX),
        Action::Open => {
            if let Some(id) = state.current_id() {
                let target = RouteTarget::Object {
                    view: state.view.clone(),
                    id: ObjectRef::Id(id.into()),
                };
                state.open(cx, target);
            }
        }
        Action::New if state.can_create => state.open(cx, RouteTarget::New(state.view.clone())),
        Action::Delete if state.can_destroy && state.has_selection() => {
            state.confirm_bulk(BulkAction::Destroy, DELETE_VERB)
        }
        Action::Delete if state.can_destroy => {
            if let Some(ids) = state.targets() {
                state.confirm_delete(ids, DELETE_VERB);
            }
        }
        Action::Menu => state.menu(cx),
        Action::New | Action::Delete => cx.flash.set(Level::Warn, READ_ONLY, cx.now),
        Action::Mark => {
            if let Some(id) = state.current_id() {
                let id: Box<str> = id.into();
                if !state.marks.remove(&id) {
                    state.marks.insert(id);
                }
                state.move_by(cx, 1);
            }
        }
        Action::MarkPage => state.mark_page(),
        Action::Sort => state.cycle_sort(cx),
        Action::Reload => {
            cx.conn.pickers.clear();
            state.reload(cx);
        }
        Action::Filter => state.filters.open(),
        Action::MarkAll => {
            state.all_matching = !state.all_matching;
            state.marks.clear();
        }
        Action::Back => {
            if let Some(running) = &state.running {
                running.cancel.cancel();
                cx.flash
                    .set(Level::Info, "Cancelling after the current batch", cx.now);
            } else if state.has_selection() {
                state.marks.clear();
                state.all_matching = false;
            } else {
                cx.back();
            }
        }
    }
}

fn done(state: &mut State, cx: &mut Cx<'_>, output: ObjectsOutput) {
    match output {
        ObjectsOutput::Page { position, result } => match result {
            Ok(page) => state.arrive(cx, position, *page),
            Err(err) => {
                let page = usize::try_from(position).unwrap_or(0) / state.pager.page_size();
                state.pager.failed(page);
                state.error = Some(err.to_string().into());
            }
        },
        ObjectsOutput::Labels { position, labels } => state.labelled(cx, position, *labels),
        ObjectsOutput::Set(Ok(response)) => state.set_done(cx, &response),
        ObjectsOutput::Set(Err(err)) => cx.flash.set(Level::Error, err.to_string(), cx.now),
        ObjectsOutput::PickerOptions(reply) => state.layers.picker_options(cx, reply),
        ObjectsOutput::Bulk(Ok(summary)) => state.bulk_done(cx, &summary),
        ObjectsOutput::Bulk(Err(err)) => {
            state.running = None;
            cx.flash.set(Level::Error, err.to_string(), cx.now);
            state.reload(cx);
        }
        ObjectsOutput::Get(_) | ObjectsOutput::BlobTexts(_) | ObjectsOutput::Tool(_) => {}
    }
}

pub fn render(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    let mut title = String::with_capacity(state.title.len() + 24);
    title.push(' ');
    title.push_str(&state.title);
    if let Some(total) = state.pager.total() {
        let _ = write!(title, " ({total})");
    }
    if let Some((label, ascending)) = state.sort_label() {
        let _ = write!(title, " · by {label} {}", if ascending { "↑" } else { "↓" });
    }
    if state.all_matching {
        title.push_str(" · all matching marked");
    } else if !state.marks.is_empty() {
        let _ = write!(title, " · {} marked", state.marks.len());
    }
    title.push(' ');
    let block = theme.panel(Line::from(title), cx.focused && state.layers.is_empty());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let bar =
        !state.filters.is_empty() && (state.filters.is_editing() || state.filters.is_active());
    let [body, filter_row, status] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(u16::from(bar)),
        Constraint::Length(1),
    ])
    .areas(inner);
    if bar {
        let list = cx
            .conn
            .schema
            .view(&state.view)
            .and_then(|view| view.list)
            .map_or(&[][..], |list| list.filters.as_slice());
        frame.render_widget(state.filters.line(theme, list), filter_row);
    }
    state.height = TableView::<fn(usize) -> _, fn(usize) -> bool>::body_height(body);
    let pager = &state.pager;
    let marks = &state.marks;
    let view = TableView {
        columns: &state.columns,
        total: pager.total().unwrap_or(0),
        rows: |pos| pager.row(pos),
        marked: |pos| pager.row(pos).is_some_and(|row| marks.contains(&row.id)),
    };
    view.render(frame, body, theme, &mut state.cursor);
    let mut line = String::with_capacity(64);
    match (&state.running, &state.error, pager.total()) {
        (Some(running), _, _) => {
            match running.progress {
                None => line.push_str("mass action starting…"),
                Some(BulkEvent::Resolving { fetched }) => {
                    let _ = write!(line, "resolving matching objects: {fetched}…");
                }
                Some(BulkEvent::Resolved { total }) => {
                    let _ = write!(line, "{total} objects selected…");
                }
                Some(BulkEvent::Batch {
                    batch,
                    done,
                    failed,
                }) => {
                    let _ = write!(
                        line,
                        "batch {} of {}: {done} done, {failed} failed…",
                        batch.number, batch.of
                    );
                }
            }
            line.push_str(" esc to cancel");
        }
        (None, Some(error), _) => line.push_str(error),
        (None, None, None) => line.push_str("loading…"),
        (None, None, Some(0)) => {
            let _ = write!(line, "No {}", state.plural);
            if !state.filters.is_active() && state.preset.is_none() {
                line.push_str(" yet");
            } else {
                line.push_str(" match");
            }
            if state.can_create {
                line.push_str(" · n to create one");
            }
        }
        (None, None, Some(total))
            if !state.all_matching && !state.marks.is_empty() && state.marks.len() < total =>
        {
            let _ = write!(
                line,
                "{} marked · * to mark all {total} matching",
                state.marks.len()
            );
        }
        (None, None, Some(total)) => {
            let _ = write!(
                line,
                "{} of {} · {} loaded",
                state.cursor.selected + 1,
                total,
                pager.loaded()
            );
        }
    }
    let style = if state.error.is_some() {
        ratatui::style::Style::new().fg(theme.err)
    } else {
        theme.muted()
    };
    frame.render_widget(Line::styled(line, style), status);
    state.layers.render(frame, area, theme);
}

pub fn crumbs(state: &State, out: &mut Crumbs) {
    out.push(&state.title);
    if let Some(crumb) = &state.preset_crumb {
        out.push(crumb);
    }
}

fn preset_crumb(
    labels: &LabelCache,
    def: Option<&ViewDef<'_>>,
    field: &str,
    value: &str,
) -> Box<str> {
    let label = def
        .and_then(|def| {
            def.list
                .and_then(|list| list.filters.iter().find(|filter| filter.field() == field))
                .map(Filter::label)
                .or_else(|| {
                    def.form_sections(None)
                        .sections
                        .iter()
                        .flat_map(|section| &section.fields)
                        .find(|entry| entry.name == field)
                        .map(|entry| entry.label)
                })
        })
        .unwrap_or(field);
    let value = def
        .and_then(|def| def.object.all_fields().get(field))
        .and_then(|field| referenced_objects(&field.typ).next())
        .and_then(|object| labels.get(object, value))
        .unwrap_or(value);
    format!("{label}: {value}").into()
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/browser/tests.rs"]
pub mod tests;
