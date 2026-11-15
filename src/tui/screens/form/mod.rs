/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

mod external;
pub mod save;

use crate::input::patch::{create_payload, update_patch};
use crate::jmap::account::{Edition, Op, Permissions};
use crate::jmap::response::SetResponse;
use crate::schema::FieldType;
use crate::tui::app::{Cx, ScreenMsg, ViewCx};
use crate::tui::exec::Slot;
use crate::tui::form::layer::{Layer, LayerEvent, Layers, Purpose};
use crate::tui::form::model::{
    EnterpriseFields, FormModel, Mode, RowKind, Rules, Shape, allowed, mask_secrets,
};
use crate::tui::form::render::{self, FormFrame, PASSKEY_HINT};
use crate::tui::form::view::{
    EDIT_KEYS, EditAction, FormAction, FormEvent, FormView, KEYS, Pick, PickKind,
};
use crate::tui::job::objects::{
    BlobSpec, BlobTexts, GetSpec, ObjectsKind, ObjectsOutput, ObjectsSlot, PickerSpec, Reply,
    SetSpec, TENANT_PROPERTY, Upload,
};
use crate::tui::keymap::{self, Context};
use crate::tui::msg::{Effect, External, HttpUrl, JobOutput};
use crate::tui::route::Route;
use crate::tui::screens::browser::filters::enum_items;
use crate::tui::screens::objects_job;
use crate::tui::session::Connected;
use crate::tui::theme::Level;
use crate::tui::widgets::chrome::Crumbs;
use crate::tui::widgets::dialog::{CONFIRM_APPLY, CONFIRM_DESTROY, CONFIRM_DISCARD, Dialog, Risk};
use crate::tui::widgets::picker::{Picker, PickerItem, Selection};
use crate::view::RefLabels;
use crate::view::set_error_plain;
use crate::view::value::ValueFormatter;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::text::Line;
use serde_json::{Map, Value, json};
use std::io;
use std::path::PathBuf;
use std::slice;
use std::time::Instant;

const NOT_SIEVE: &str = "Sievepad links are only available on Sieve script fields";
const CREATE_ID: &str = "new";
pub const SINGLETON_ID: &str = "singleton";
const JSON_TOGGLE: &str = "diff / JSON";
const WEB_PATH: &str = "/admin";
const PERMISSION_ENUM: &str = "Permission";
const NO_RESULT: &str = "Done. No additional data returned.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Create,
    Edit(Box<str>),
    Singleton,
    Action(Box<str>),
}

impl Target {
    fn id(&self) -> &str {
        match self {
            Target::Create | Target::Action(_) => CREATE_ID,
            Target::Edit(id) => id,
            Target::Singleton => SINGLETON_ID,
        }
    }

    fn is_create(&self) -> bool {
        matches!(self, Target::Create | Target::Action(_))
    }
}

pub struct State {
    view: Box<str>,
    object: Box<str>,
    singular: Box<str>,
    heading: Option<Box<str>>,
    title: Box<str>,
    label: Option<Box<str>>,
    target: Target,
    rules: Rules,
    form: Option<FormView>,
    layers: Layers,
    status: Box<str>,
    external: Option<PathBuf>,
    closed: bool,
    blobs: Vec<Box<str>>,
    blob_values: BlobValues,
    pending: Option<Map<String, Value>>,
    resume: Option<(usize, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlobValues {
    Ids,
    Texts,
}

impl State {
    pub fn new(conn: &Connected, view: &str, target: Target) -> Self {
        let schema = &conn.schema;
        let def = schema.view(view);
        let list = def.as_ref().and_then(|def| def.list);
        let object: Box<str> = def.as_ref().map_or(view, |def| def.object.name()).into();
        let singular: Box<str> = list
            .map_or("object", |list| list.singular_name.as_str())
            .into();
        let mut rules = Rules::new(if target.is_create() {
            Mode::Create
        } else {
            Mode::Edit
        });
        rules.mutable_only = matches!(target, Target::Action(_));
        if conn.info.edition == Edition::Community {
            rules.enterprise = EnterpriseFields::Locked;
        }
        rules.read_only =
            !target.is_create() && !allowed(schema, &conn.info.permissions, view, Op::Update);
        let variant = match &target {
            Target::Action(name) => def.as_ref().and_then(|def| def.object.variant(name)),
            Target::Create | Target::Edit(_) | Target::Singleton => None,
        };
        let title = match (&target, variant) {
            (Target::Create, _) => format!("New {singular}"),
            (Target::Action(_), Some(variant)) => variant.label.clone(),
            (Target::Action(name), None) => name.to_string(),
            (Target::Edit(_) | Target::Singleton, _) => singular.to_string(),
        };
        let mut state = State {
            view: view.into(),
            object,
            heading: list.map(|list| list.title.as_str().into()),
            singular,
            title: title.into(),
            label: None,
            target,
            rules,
            form: None,
            layers: Layers::default(),
            status: "loading…".into(),
            external: None,
            closed: false,
            blobs: def
                .as_ref()
                .map(|def| {
                    def.object
                        .all_fields()
                        .all_defs()
                        .filter(|(_, field)| matches!(field.typ, FieldType::BlobId))
                        .map(|(name, _)| name.into())
                        .collect()
                })
                .unwrap_or_default(),
            blob_values: if conn.active_account.is_some() {
                BlobValues::Texts
            } else {
                BlobValues::Ids
            },
            pending: None,
            resume: None,
        };
        match &def {
            None => state.status = "This view is not in the schema.".into(),
            Some(def) if state.target.is_create() => {
                let values = schema.create_defaults(def, variant);
                state.build(conn, values.clone(), values);
            }
            Some(_) => {}
        }
        state
    }

    pub fn start(&mut self, cx: &mut Cx<'_>) {
        if self.target.is_create() || cx.conn.schema.view(&self.view).is_none() {
            return;
        }
        let spec = GetSpec {
            object: self.object.clone(),
            id: self.target.id().into(),
            properties: None,
        };
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Load));
        cx.fx
            .spawn(ticket, objects_job(cx, ObjectsKind::Get(Box::new(spec))));
    }

    fn build(
        &mut self,
        conn: &Connected,
        original: Map<String, Value>,
        values: Map<String, Value>,
    ) {
        let schema = &conn.schema;
        let permissions = &conn.info.permissions;
        let can_get = |name: &str| allowed(schema, permissions, name, Op::Get);
        let model = FormModel::new(
            schema,
            Shape::View(self.view.clone()),
            self.rules,
            original,
            values,
            &can_get,
        );
        if self.target == Target::Singleton
            && let Some(title) = &model.layout.title
        {
            self.title = title.clone();
        }
        let mut form = FormView::at_input(model);
        if let Some((row, scroll)) = self.resume.take()
            && row < form.model.layout.rows.len()
        {
            form.row = row;
            form.scroll = scroll;
        }
        self.form = Some(form);
    }

    fn loaded(&mut self, cx: &mut Cx<'_>, result: Reply<Option<Map<String, Value>>>) {
        let object = match result {
            Ok(object) => match *object {
                Some(object) => object,
                None => {
                    self.status = "This object no longer exists.".into();
                    return;
                }
            },
            Err(err) => {
                self.status = err.to_string().into();
                return;
            }
        };
        let schema = cx.conn.schema.clone();
        if let Target::Edit(id) = &self.target {
            let label = schema
                .view(&self.view)
                .and_then(|def| def.label_property())
                .and_then(|property| object.get(property))
                .and_then(Value::as_str)
                .filter(|label| !label.is_empty())
                .unwrap_or(id);
            let verb = if self.rules.read_only { "" } else { "Edit " };
            self.title = format!("{verb}{} {label}", self.singular).into();
            self.label = Some(label.into());
        }
        if let Some(def) = schema.get_object(&self.object) {
            let request =
                cx.conn
                    .labels
                    .missing(&schema, def.all_fields(), slice::from_ref(&object));
            if !request.is_empty() {
                let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Labels(0)));
                let kind = ObjectsKind::Labels {
                    position: 0,
                    request: Box::new(request),
                };
                cx.fx.spawn(ticket, objects_job(cx, kind));
            }
        }
        let blobs: Vec<(Box<str>, Box<str>)> = match self.blob_values {
            BlobValues::Texts => self
                .blobs
                .iter()
                .filter_map(|name| Some((name.clone(), object.get(&**name)?.as_str()?.into())))
                .collect(),
            BlobValues::Ids => Vec::new(),
        };
        if blobs.is_empty() {
            self.build(cx.conn, object.clone(), object);
            return;
        }
        self.pending = Some(object);
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Load));
        let kind = ObjectsKind::BlobTexts(Box::new(BlobSpec { blobs }));
        cx.fx.spawn(ticket, objects_job(cx, kind));
    }

    fn blobs_loaded(&mut self, cx: &mut Cx<'_>, result: Reply<BlobTexts>) {
        let Some(mut object) = self.pending.take() else {
            return;
        };
        match result {
            Ok(texts) => {
                for (name, text) in *texts {
                    object.insert(name.into(), text.map_or(Value::Null, Value::String));
                }
            }
            Err(err) => {
                self.blob_values = BlobValues::Ids;
                cx.flash.set(Level::Error, err.to_string(), cx.now);
            }
        }
        self.build(cx.conn, object.clone(), object);
    }

    fn pick(&mut self, cx: &mut Cx<'_>, pick: Pick) {
        let Some(root) = &self.form else {
            return;
        };
        let form = root.active();
        let Some((label, current)) = root.pick_target(&pick) else {
            return;
        };
        let schema = cx.conn.schema.clone();
        let selection = |multi: bool| {
            if multi {
                Selection::Multi
            } else {
                Selection::Single
            }
        };
        let mut load = None;
        let picker = match &pick.kind {
            PickKind::Variant => {
                let items = form
                    .model
                    .shape
                    .variants(&schema)
                    .iter()
                    .map(|variant| {
                        let label = if variant.label.is_empty() {
                            &variant.name
                        } else {
                            &variant.label
                        };
                        PickerItem::new(variant.name.as_str(), label.as_str(), "")
                    })
                    .collect();
                Picker::new(label, items, Selection::Single).with_current(form.model.at_type())
            }
            PickKind::Enum {
                name,
                nullable,
                multi,
            } => {
                let mut items = enum_items(&schema, name);
                if &**name == PERMISSION_ENUM {
                    retain_held(&mut items, &cx.conn.info.permissions, current);
                }
                if *nullable && !*multi {
                    items.insert(0, PickerItem::new("", "None", "clear the value"));
                }
                Picker::new(label, items, selection(*multi))
            }
            PickKind::Reference { object, multi, .. } => {
                let tenant = root.model.value(TENANT_PROPERTY).and_then(Value::as_str);
                load = Some(PickerSpec::new(&schema, object, tenant));
                Picker::loading(label, selection(*multi))
            }
        };
        let picker = match (&pick.kind, current) {
            (PickKind::Variant, _) => picker,
            (kind, Some(Value::Object(set))) => {
                let object = match kind {
                    PickKind::Reference { object, .. } => Some(&**object),
                    PickKind::Enum { .. } | PickKind::Variant => None,
                };
                let labels = &cx.conn.labels;
                picker.with_marked(set.keys().map(|key| {
                    let label = object.and_then(|object| labels.get(object, key));
                    (key.as_str(), label.unwrap_or(key))
                }))
            }
            (_, value) => picker.with_current(value.and_then(Value::as_str)),
        };
        self.layers.push(Layer::Picker {
            picker,
            purpose: Purpose::FieldPick(Box::new(pick)),
        });
        if let Some(spec) = load {
            self.layers.load_picker(cx, spec);
        }
    }

    fn picked(&mut self, cx: &mut Cx<'_>, pick: &Pick, items: Vec<(Box<str>, Box<str>)>) {
        let Some(form) = &mut self.form else {
            return;
        };
        if let PickKind::Reference { object, .. } = &pick.kind {
            for (id, label) in items.iter().filter(|(id, label)| id != label) {
                cx.conn.labels.insert(object, id, label);
            }
        }
        let keys: Vec<Box<str>> = items.into_iter().map(|(key, _)| key).collect();
        let schema = &cx.conn.schema;
        let permissions = &cx.conn.info.permissions;
        let can_get = |name: &str| allowed(schema, permissions, name, Op::Get);
        form.apply_pick(schema, &can_get, pick, &keys);
    }

    fn review(&mut self, cx: &mut Cx<'_>) {
        let schema = cx.conn.schema.clone();
        let Some(form) = &mut self.form else {
            return;
        };
        if form.model.rules.read_only {
            cx.flash
                .set(Level::Warn, "Read-only for this account", cx.now);
            return;
        }
        form.model.banner = None;
        if !form.model.validate(&schema) {
            if let Some(row) = form.model.first_error_row() {
                form.row = row;
            }
            let text = format!("{} fields need attention", form.model.error_count());
            cx.flash.set(Level::Error, text, cx.now);
            return;
        }
        let Some(def) = schema.view(&self.view) else {
            return;
        };
        let fields = def.object.all_fields();
        let model = &form.model;
        let mut shown = model.values.clone();
        mask_secrets(&schema, fields, &mut shown);
        let texts = self.blob_values == BlobValues::Texts;
        let cells = save::Cells {
            formatter: ValueFormatter::new(&schema, &cx.conn.labels),
            fields,
        };
        let (args, shown_args, uploads, body, risk) = match &self.target {
            Target::Create | Target::Action(_) => {
                let mut payload = create_payload(&model.values, fields, def.static_filter);
                let mut shown = create_payload(&shown, fields, def.static_filter);
                let body = save::create_lines(cells, &self.singular, &shown);
                let uploads = if texts {
                    save::blob_uploads(&mut shown, &self.blobs);
                    save::blob_uploads(&mut payload, &self.blobs)
                } else {
                    Vec::new()
                };
                (
                    json!({ "create": { CREATE_ID: payload } }),
                    json!({ "create": { CREATE_ID: shown } }),
                    uploads,
                    body,
                    Risk::Low,
                )
            }
            Target::Edit(_) | Target::Singleton => {
                let mut patch = update_patch(&model.original, &model.values, fields);
                if patch.is_empty() {
                    cx.flash.set(Level::Info, "No changes", cx.now);
                    return;
                }
                let mut shown = update_patch(&model.original, &shown, fields);
                let id = self.target.id();
                let name = match (&self.target, &self.label) {
                    (Target::Singleton, _) => self.title.clone(),
                    (_, Some(label)) => format!("{} {label}", self.singular).into(),
                    _ => format!("{} {id}", self.singular).into(),
                };
                let body = save::update_lines(cells, &name, &model.original, &shown);
                let uploads = if texts {
                    save::blob_uploads(&mut shown, &self.blobs);
                    save::blob_uploads(&mut patch, &self.blobs)
                } else {
                    Vec::new()
                };
                let risk = if save::variant_changed(&model.original, &model.values) {
                    Risk::High
                } else {
                    Risk::Low
                };
                (
                    json!({ "update": { id: patch } }),
                    json!({ "update": { id: shown } }),
                    uploads,
                    body,
                    risk,
                )
            }
        };
        let json = save::json_lines(&self.object, &shown_args);
        self.layers.push(Layer::Dialog {
            dialog: Dialog::confirm("Review changes", risk, body, CONFIRM_APPLY)
                .with_alternate(json, JSON_TOGGLE),
            purpose: Purpose::Save(Box::new((args, uploads))),
        });
    }

    fn send(&mut self, cx: &mut Cx<'_>, args: Value, uploads: Vec<Upload>) {
        let spec = SetSpec {
            object: self.object.clone(),
            args,
            uploads,
        };
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Save));
        cx.fx
            .spawn(ticket, objects_job(cx, ObjectsKind::Set(Box::new(spec))));
    }

    fn saved(&mut self, cx: &mut Cx<'_>, response: SetResponse) {
        cx.conn.pickers.forget(&self.object);
        let id = self.target.id();
        if let Some((_, err)) = response.not_destroyed.first() {
            let text = set_error_plain(err, &RefLabels::default());
            cx.flash.set(Level::Error, text, cx.now);
            return;
        }
        if response.destroyed.iter().any(|destroyed| destroyed == id) {
            cx.flash.set(Level::Ok, "Deleted", cx.now);
            cx.router.back();
            return;
        }
        let failures = match self.target {
            Target::Create | Target::Action(_) => &response.not_created,
            Target::Edit(_) | Target::Singleton => &response.not_updated,
        };
        if let Some((_, err)) = failures.iter().find(|(failed, _)| *failed == id) {
            if let Some(form) = &mut self.form {
                let text = set_error_plain(err, &RefLabels::default());
                form.model.apply_set_error(err, &text);
                form.row = form.model.first_error_row().unwrap_or(0);
            }
            cx.flash.set(Level::Error, "Save failed", cx.now);
            return;
        }
        let created = response.created.get(CREATE_ID);
        if let Target::Action(variant) = &self.target {
            let schema = &cx.conn.schema;
            let input = self.form.as_ref().map(|form| {
                let mut input = form.model.values.clone();
                if let Some(def) = schema.view(&self.view) {
                    mask_secrets(schema, def.object.all_fields(), &mut input);
                }
                input
            });
            let result =
                input.and_then(|input| save::action_result(&input, created, Some(variant)));
            show_result(&mut self.layers, &self.object, result, created);
            return;
        }
        if self.target == Target::Create
            && let Some(text) = save::created_text(created)
        {
            let at_type = self.form.as_ref().and_then(|form| form.model.at_type());
            let doc = save::created_doc(created, at_type).unwrap_or_default();
            self.layers
                .push_doc("Created", &self.object, doc, Purpose::Created(text.into()));
            return;
        }
        let text = match self.target {
            Target::Create => "Created",
            Target::Action(_) => "Done",
            Target::Edit(_) | Target::Singleton => "Saved",
        };
        cx.flash.set(Level::Ok, text, cx.now);
        self.finish(cx);
    }

    fn finish(&mut self, cx: &mut Cx<'_>) {
        match self.target {
            Target::Singleton => {
                self.resume = self.form.take().map(|form| (form.row, form.scroll));
                self.status = "loading…".into();
                self.start(cx);
            }
            Target::Create | Target::Edit(_) | Target::Action(_) => self.exit(cx),
        }
    }

    pub fn closed(&self) -> bool {
        self.closed
    }

    pub fn has_fields(&self) -> bool {
        self.form.as_ref().is_some_and(|form| {
            form.model
                .layout
                .rows
                .iter()
                .any(|row| row.kind == RowKind::Field)
        })
    }

    fn exit(&mut self, cx: &mut Cx<'_>) {
        if matches!(self.target, Target::Action(_)) {
            self.closed = true;
        } else {
            cx.back();
        }
    }

    fn link(&mut self, cx: &mut Cx<'_>, action: FormAction) {
        let Some(form) = &self.form else {
            return;
        };
        let schema = &cx.conn.schema;
        let link = match form.sievepad(schema) {
            Some(Ok(link)) => link,
            Some(Err(err)) => {
                cx.flash.set(Level::Error, err.to_string(), cx.now);
                return;
            }
            None if form.on_passkey(schema) => web_url(&cx.conn.url),
            None if action == FormAction::Copy => {
                match form.copy_text() {
                    Some(text) => {
                        cx.fx.push(Effect::Copy(text.into_boxed_str()));
                        cx.flash.set(Level::Ok, "Copied", cx.now);
                    }
                    None => cx.flash.set(Level::Warn, "Nothing to copy", cx.now),
                }
                return;
            }
            None => {
                cx.flash.set(Level::Warn, NOT_SIEVE, cx.now);
                return;
            }
        };
        if action == FormAction::Copy {
            cx.fx.push(Effect::Copy(link.into_boxed_str()));
            cx.flash.set(Level::Ok, "Link copied", cx.now);
        } else if let Some(url) = HttpUrl::parse(&link) {
            cx.fx.push(Effect::OpenUrl(url));
        }
    }

    fn dirty(&self) -> bool {
        self.form
            .as_ref()
            .is_some_and(|form| !form.model.rules.read_only && form.has_changes())
    }

    fn leave(&mut self, cx: &mut Cx<'_>) {
        if self.dirty() {
            self.confirm_discard(Purpose::Discard);
        } else {
            self.exit(cx);
        }
    }

    fn passkey_notice(&mut self, cx: &Cx<'_>) {
        let body = vec![
            Line::raw(PASSKEY_HINT),
            Line::raw(""),
            Line::raw(web_url(&cx.conn.url)),
            Line::raw(""),
            Line::raw(
                "A terminal cannot run the WebAuthn ceremony; o on the field opens the page.",
            ),
        ];
        self.layers.push(Layer::Dialog {
            dialog: Dialog::info("Passkeys", body),
            purpose: Purpose::Notice,
        });
    }

    fn confirm_discard(&mut self, purpose: Purpose) {
        let body = vec![Line::raw(
            "Leave this form and discard the unsaved changes?",
        )];
        self.layers.push(Layer::Dialog {
            dialog: Dialog::confirm("Unsaved changes", Risk::High, body, CONFIRM_DISCARD),
            purpose,
        });
    }

    fn confirm_delete(&mut self, cx: &mut Cx<'_>) {
        let Target::Edit(id) = &self.target else {
            return;
        };
        if !allowed(
            &cx.conn.schema,
            &cx.conn.info.permissions,
            &self.view,
            Op::Destroy,
        ) {
            cx.flash
                .set(Level::Warn, "Not permitted for this account", cx.now);
            return;
        }
        let ids: Box<[Box<str>]> = Box::new([id.clone()]);
        let label = self.label.as_deref().unwrap_or(id);
        let body = vec![
            Line::raw(format!("Delete {} {label}?", self.singular)),
            Line::raw(""),
            Line::raw(format!(
                "{}/set {}",
                self.object,
                json!({ "destroy": &*ids })
            )),
        ];
        self.layers.push(Layer::Dialog {
            dialog: Dialog::confirm("Delete", Risk::High, body, CONFIRM_DESTROY),
            purpose: Purpose::Destroy(ids),
        });
    }

    fn open_external(&mut self, cx: &mut Cx<'_>) {
        let Some(form) = &self.form else {
            return;
        };
        if form.model.rules.read_only {
            cx.flash
                .set(Level::Warn, "Read-only for this account", cx.now);
            return;
        }
        let text = match serde_json::to_string_pretty(&form.model.values) {
            Ok(text) => text,
            Err(err) => {
                cx.flash.set(Level::Error, err.to_string(), cx.now);
                return;
            }
        };
        match external::write(&text) {
            Ok(path) => {
                let ticket = cx.slots.start(Slot::Editor);
                cx.fx.push(Effect::Suspend(External::Editor {
                    path: path.clone(),
                    ticket,
                }));
                self.external = Some(path);
            }
            Err(err) => cx.flash.set(Level::Error, err.to_string(), cx.now),
        }
    }

    fn resumed(&mut self, cx: &mut Cx<'_>, error: Option<&io::Error>) {
        let Some(path) = self.external.take() else {
            return;
        };
        let text = external::take(&path);
        if let Some(err) = error {
            cx.flash.set(Level::Error, err.to_string(), cx.now);
            return;
        }
        let values = match text.map_err(|err| err.to_string()).and_then(|text| {
            serde_json::from_str::<Map<String, Value>>(&text).map_err(|err| err.to_string())
        }) {
            Ok(values) => values,
            Err(err) => {
                cx.flash
                    .set(Level::Error, format!("JSON not applied: {err}"), cx.now);
                return;
            }
        };
        let Some(form) = &mut self.form else {
            return;
        };
        let schema = &cx.conn.schema;
        let permissions = &cx.conn.info.permissions;
        let can_get = |name: &str| allowed(schema, permissions, name, Op::Get);
        form.model.values = values;
        form.model.banner = None;
        form.model.relayout(schema, &can_get);
        let valid = form.model.validate(schema);
        form.row = form.model.first_error_row().unwrap_or(0);
        let (level, text) = if valid {
            (Level::Info, "JSON applied to the form; ^s to review")
        } else {
            (
                Level::Warn,
                "JSON applied with errors; fix them before saving",
            )
        };
        cx.flash.set(level, text, cx.now);
    }

    fn layer_event(&mut self, cx: &mut Cx<'_>, event: LayerEvent) {
        match event {
            LayerEvent::Chosen {
                purpose: Purpose::FieldPick(pick),
                items,
            } => self.picked(cx, &pick, items),
            LayerEvent::Confirmed(Purpose::Save(request)) => {
                let (args, uploads) = *request;
                self.send(cx, args, uploads)
            }
            LayerEvent::Confirmed(Purpose::Destroy(ids)) => {
                self.send(cx, json!({ "destroy": ids }), Vec::new())
            }
            LayerEvent::Confirmed(Purpose::Discard) => self.exit(cx),
            LayerEvent::Confirmed(Purpose::Leave(route)) => cx.router.jump(*route),
            LayerEvent::Confirmed(Purpose::Reload) => cx.router.reload(),
            LayerEvent::Confirmed(Purpose::Created(text)) => {
                if !text.is_empty() {
                    cx.fx.push(Effect::Copy(text));
                }
                self.finish(cx);
            }
            LayerEvent::Closed(Purpose::Created(_)) => self.finish(cx),
            LayerEvent::Pending
            | LayerEvent::Closed(_)
            | LayerEvent::Confirmed(_)
            | LayerEvent::Chosen { .. } => {}
        }
    }
}

pub fn show_result(
    layers: &mut Layers,
    object: &str,
    result: Option<Value>,
    created: Option<&Value>,
) {
    match result {
        Some(doc) => {
            let text = save::created_text(created)
                .or_else(|| serde_json::to_string_pretty(&doc).ok())
                .unwrap_or_default();
            layers.push_doc("Result", object, doc, Purpose::Created(text.into()));
        }
        None => layers.push(Layer::Dialog {
            dialog: Dialog::info("Result", vec![Line::raw(NO_RESULT)]),
            purpose: Purpose::Created(Box::default()),
        }),
    }
}

fn retain_held(items: &mut Vec<PickerItem>, held: &Permissions, current: Option<&Value>) {
    items.retain(|item| {
        held.has(&item.key)
            || match current {
                Some(Value::Object(set)) => set.contains_key(&*item.key),
                Some(Value::String(chosen)) => **chosen == *item.key,
                _ => false,
            }
    });
}

fn web_url(server: &str) -> String {
    let mut url = String::with_capacity(server.len() + WEB_PATH.len());
    url.push_str(server.trim_end_matches('/'));
    url.push_str(WEB_PATH);
    url
}

pub fn context(state: &State) -> Context {
    match state.layers.context() {
        Some(ctx) => ctx,
        None => state
            .form
            .as_ref()
            .and_then(FormView::edit_context)
            .unwrap_or(Context::Form),
    }
}

pub fn guard_leave(state: &mut State, route: Route) -> Option<Route> {
    if state.dirty() {
        state.confirm_discard(Purpose::Leave(Box::new(route)));
        None
    } else {
        Some(route)
    }
}

pub fn deadline(state: &State) -> Option<Instant> {
    state.layers.deadline()
}

pub fn guard_reload(state: &mut State) -> bool {
    if state.dirty() {
        state.confirm_discard(Purpose::Reload);
        false
    } else {
        true
    }
}

pub fn is_dirty(state: &State) -> bool {
    state.dirty()
}

pub fn handle_key(state: &mut State, cx: &mut Cx<'_>, event: KeyEvent) -> bool {
    if !state.layers.is_empty() {
        let event = state.layers.handle(event);
        state.layers.stamp(cx.now);
        state.layer_event(cx, event);
        return true;
    }
    if let Some(form) = &mut state.form
        && form.is_editing()
    {
        let action = keymap::resolve(EDIT_KEYS, event);
        let schema = &cx.conn.schema;
        let permissions = &cx.conn.info.permissions;
        let can_get = |name: &str| allowed(schema, permissions, name, Op::Get);
        if action == Some(EditAction::Copy) {
            if let Some(text) = form.copy_text() {
                cx.fx.push(Effect::Copy(text.into_boxed_str()));
                cx.flash.set(Level::Ok, "Copied", cx.now);
            }
            return true;
        }
        form.edit_key(schema, &can_get, event, action);
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

pub fn update(state: &mut State, cx: &mut Cx<'_>, msg: ScreenMsg<'_, FormAction>) {
    match msg {
        ScreenMsg::Key(action) => act(state, cx, action),
        ScreenMsg::Paste(text) if state.layers.is_empty() => {
            if let Some(form) = &mut state.form {
                form.paste(text);
            }
        }
        ScreenMsg::Paste(text) => {
            state.layers.paste(text);
            state.layers.stamp(cx.now);
        }
        ScreenMsg::Tick(now) => state.layers.tick(cx, now),
        ScreenMsg::Done(JobOutput::Objects(output)) => done(state, cx, output),
        ScreenMsg::Resumed(error) => state.resumed(cx, error),
        ScreenMsg::Progress(_) => {}
        ScreenMsg::Text(_) | ScreenMsg::Done(_) | ScreenMsg::Stream(_) => {}
    }
}

fn act(state: &mut State, cx: &mut Cx<'_>, action: FormAction) {
    if matches!(action, FormAction::Open | FormAction::Copy) {
        state.link(cx, action);
        return;
    }
    let schema = cx.conn.schema.clone();
    let Some(form) = &mut state.form else {
        if action == FormAction::Back {
            state.exit(cx);
        }
        return;
    };
    let permissions = &cx.conn.info.permissions;
    let can_get = |name: &str| allowed(&schema, permissions, name, Op::Get);
    match form.act(&schema, &can_get, action) {
        FormEvent::None => {}
        FormEvent::Pick(pick) => state.pick(cx, pick),
        FormEvent::Save => state.review(cx),
        FormEvent::External => state.open_external(cx),
        FormEvent::Delete => state.confirm_delete(cx),
        FormEvent::Passkey => state.passkey_notice(cx),
        FormEvent::Back => state.leave(cx),
    }
}

fn done(state: &mut State, cx: &mut Cx<'_>, output: ObjectsOutput) {
    match output {
        ObjectsOutput::Get(result) => state.loaded(cx, result),
        ObjectsOutput::Labels { labels, .. } => cx.conn.labels.insert_all(*labels),
        ObjectsOutput::Set(Ok(response)) => state.saved(cx, *response),
        ObjectsOutput::Set(Err(err)) => cx.flash.set(Level::Error, err.to_string(), cx.now),
        ObjectsOutput::PickerOptions(reply) => state.layers.picker_options(cx, reply),
        ObjectsOutput::BlobTexts(result) => state.blobs_loaded(cx, result),
        ObjectsOutput::Page { .. } | ObjectsOutput::Tool(_) | ObjectsOutput::Bulk(_) => {}
    }
}

pub fn render(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    match &mut state.form {
        Some(form) => render::render(
            form,
            frame,
            area,
            &FormFrame {
                title: &state.title,
                schema: &cx.conn.schema,
                labels: &cx.conn.labels,
                theme,
                focused: cx.focused && state.layers.is_empty(),
            },
        ),
        None => {
            let block = theme.panel(Line::from(&*state.title), cx.focused);
            let inner = block.inner(area);
            frame.render_widget(block, area);
            frame.render_widget(Line::styled(&*state.status, theme.muted()), inner);
        }
    }
    state.layers.build_docs(cx);
    state.layers.render(frame, area, theme);
}

pub fn crumbs(state: &State, out: &mut Crumbs) {
    match &state.target {
        Target::Singleton | Target::Action(_) => out.push(&state.title),
        Target::Create => {
            if let Some(heading) = &state.heading {
                out.push(heading);
            }
            out.push("New");
        }
        Target::Edit(id) => {
            if let Some(heading) = &state.heading {
                out.push(heading);
            }
            out.push(state.label.as_deref().unwrap_or(id));
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/form/tests.rs"]
mod tests;

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/form/test_support.rs"]
mod test_support;
