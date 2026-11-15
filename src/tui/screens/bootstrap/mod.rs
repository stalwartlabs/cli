/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::account::{Op, Permissions};
use crate::jmap::response::SetResponse;
use crate::jmap::set_error::SetError;
use crate::schema::Schema;
use crate::tui::app::{CONFIRM_LOGOUT, Confirmable, Cx, Overlay, ScreenMsg, ViewCx};
use crate::tui::exec::Slot;
use crate::tui::form::layer::{Layer, LayerEvent, Layers, Purpose};
use crate::tui::form::model::{
    EnterpriseFields, FormModel, Layout, Mode, Rules, Section, Shape, allowed,
};
use crate::tui::form::render::{self, FormFrame};
use crate::tui::form::view::{EDIT_KEYS, FormAction, FormEvent, FormView, Pick, PickKind};
use crate::tui::job::objects::{GetSpec, ObjectsKind, ObjectsOutput, ObjectsSlot, Reply, SetSpec};
use crate::tui::keymap::{self, Context, code, ctrl, key, keys, upper};
use crate::tui::msg::{Effect, JobOutput};
use crate::tui::screens::browser::filters::enum_items;
use crate::tui::screens::form::SINGLETON_ID;
use crate::tui::screens::objects_job;
use crate::tui::theme::Level;
use crate::tui::widgets::chrome::Crumbs;
use crate::tui::widgets::dialog::{Dialog, Risk};
use crate::tui::widgets::picker::{Picker, PickerItem, Selection};
use crate::view::RefLabels;
use crate::view::set_error_plain;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use serde_json::{Map, Value, json};

pub const VIEW: &str = "x:Bootstrap";
const TITLE: &str = "Welcome to Stalwart";
const ID: &str = "id";
const HIDDEN_EXTRAS: [&str; 3] = [ID, "blobId", "@type"];
const CORRECT_ERRORS: &str = "Please correct the errors below.";
const NO_CONFIRM: &str = "The server did not confirm the update.";
const COMPLETE_WITH_CREDENTIALS: &str = "Your administrator account has been created. Write these \
     down now: the password will not be shown again.";
const COMPLETE_WITHOUT_CREDENTIALS: &str = "Stalwart has been configured successfully.";
const NEXT_WITH_CREDENTIALS: &str = "Restart Stalwart for the new configuration to take effect. \
     Once restarted, sign in with the credentials above to continue administering your server.";
const NEXT_WITHOUT_CREDENTIALS: &str = "Restart Stalwart for the new configuration to take \
     effect. No administrator account was created because accounts come from the external \
     directory. Once restarted, sign in with the recovery administrator set in \
     STALWART_RECOVERY_ADMIN and assign an administrator role to a directory account.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    Edit,
    Toggle,
    Revert,
    Clear,
    Add,
    MoveUp,
    MoveDown,
    Back,
    Next,
    Prev,
    Finish,
    Copy,
    SignIn,
    Retry,
}

keys! { pub KEYS, pub HINTS: Action;
    "j/k" Down "move" true [key('j'), code(KeyCode::Down)],
    "k" Up "up" false [key('k'), code(KeyCode::Up)],
    "⏎" Edit "edit / choose" true [code(KeyCode::Enter)],
    "space" Toggle "toggle" true [code(KeyCode::Char(' '))],
    "u" Revert "revert field" false [key('u')],
    "x" Clear "clear field" false [key('x')],
    "a" Add "add list item" false [key('a')],
    "K" MoveUp "move item up" false [upper('K')],
    "J" MoveDown "move item down" false [upper('J')],
    "esc" Back "close nested" false [code(KeyCode::Esc)],
    "tab" Next "next step" true [code(KeyCode::Tab), code(KeyCode::Right), key('n')],
    "⇧tab" Prev "previous step" true [code(KeyCode::BackTab), code(KeyCode::Left), key('p')],
    "^s" Finish "finish setup" true [ctrl('s'), upper('S')],
    "r" Retry "retry" false [key('r')],
}

keys! { pub COMPLETE_KEYS, pub COMPLETE_HINTS: Action;
    "j/k" Down "move" true [key('j'), code(KeyCode::Down)],
    "k" Up "up" false [key('k'), code(KeyCode::Up)],
    "y" Copy "copy value" true [key('y'), code(KeyCode::Enter)],
    "l" SignIn "sign in again" true [key('l')],
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Loading,
    Failed(Box<str>),
    Empty,
    Editing,
    Saving,
    Complete(Vec<Extra>),
}

#[derive(Debug, Clone, PartialEq)]
struct Extra {
    label: Box<str>,
    value: Box<str>,
}

pub struct State {
    phase: Phase,
    form: Option<FormView>,
    pages: Vec<Layout>,
    full: Layout,
    page: usize,
    cursor: usize,
    layers: Layers,
}

impl State {
    pub fn new() -> Self {
        State {
            phase: Phase::Loading,
            form: None,
            pages: Vec::new(),
            full: Layout::default(),
            page: 0,
            cursor: 0,
            layers: Layers::default(),
        }
    }

    pub fn start(&mut self, cx: &mut Cx<'_>) {
        let schema = &cx.conn.schema;
        let properties: Box<[Box<str>]> = match schema.view(VIEW) {
            Some(def) => {
                let mut names: Vec<Box<str>> = vec![ID.into()];
                names.extend(
                    def.object
                        .all_fields()
                        .all_defs()
                        .filter(|(_, field)| !field.is_server_set() && !field.enterprise)
                        .map(|(name, _)| Box::from(name)),
                );
                names.sort_unstable();
                names.dedup();
                names.into_boxed_slice()
            }
            None => {
                self.phase = Phase::Failed("This server does not offer the setup wizard.".into());
                return;
            }
        };
        let spec = GetSpec {
            object: VIEW.into(),
            id: SINGLETON_ID.into(),
            properties: Some(properties),
        };
        self.phase = Phase::Loading;
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Load));
        cx.fx
            .spawn(ticket, objects_job(cx, ObjectsKind::Get(Box::new(spec))));
    }

    fn loaded(&mut self, cx: &mut Cx<'_>, result: Reply<Option<Map<String, Value>>>) {
        let loaded = match result {
            Ok(object) => object.unwrap_or_default(),
            Err(err) => {
                self.phase = Phase::Failed(format!("Failed to load the setup: {err}").into());
                return;
            }
        };
        let schema = &cx.conn.schema;
        let Some(def) = schema.view(VIEW) else {
            return;
        };
        let mut values = schema.create_defaults(&def, None);
        merge(&mut values, loaded);
        values.remove(ID);
        let mut rules = Rules::new(Mode::Create);
        rules.enterprise = EnterpriseFields::Hidden;
        let model = FormModel::new(
            schema,
            Shape::View(VIEW.into()),
            rules,
            Map::new(),
            values,
            &every_object,
        );
        self.pages = pages(&model.layout);
        self.full = model.layout.clone();
        if self.pages.is_empty() {
            self.phase = Phase::Empty;
            return;
        }
        self.form = Some(FormView::new(model));
        self.phase = Phase::Editing;
        self.show(0);
    }

    fn show(&mut self, page: usize) {
        let (Some(form), Some(layout)) = (&mut self.form, self.pages.get(page)) else {
            return;
        };
        form.collapse();
        self.page = page;
        form.model.layout = layout.clone();
        form.row = form.model.first_error_row().unwrap_or(0);
        form.scroll = 0;
    }

    fn validate_page(&mut self, schema: &Schema) -> bool {
        let Some(form) = &mut self.form else {
            return false;
        };
        form.collapse();
        if form.model.validate(schema) {
            form.model.banner = None;
            return true;
        }
        form.model.banner = Some(CORRECT_ERRORS.into());
        form.row = form.model.first_error_row().unwrap_or(0);
        false
    }

    fn next(&mut self, cx: &mut Cx<'_>) {
        if self.page + 1 >= self.pages.len() {
            self.finish(cx);
        } else if self.validate_page(&cx.conn.schema) {
            self.show(self.page + 1);
        }
    }

    fn prev(&mut self) {
        if let Some(form) = &mut self.form {
            form.model.clear_errors();
        }
        self.show(self.page.saturating_sub(1));
    }

    fn finish(&mut self, cx: &mut Cx<'_>) {
        for page in 0..self.pages.len() {
            self.show(page);
            if !self.validate_page(&cx.conn.schema) {
                return;
            }
        }
        self.show(self.pages.len().saturating_sub(1));
        let Some(form) = &self.form else {
            return;
        };
        let spec = SetSpec {
            object: VIEW.into(),
            args: json!({ "update": { SINGLETON_ID: form.model.values } }),
            uploads: Vec::new(),
        };
        self.phase = Phase::Saving;
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Save));
        cx.fx
            .spawn(ticket, objects_job(cx, ObjectsKind::Set(Box::new(spec))));
    }

    fn saved(&mut self, cx: &mut Cx<'_>, result: Reply<SetResponse>) {
        self.phase = Phase::Editing;
        cx.conn.pickers.clear();
        let mut response = match result {
            Ok(response) => *response,
            Err(err) => {
                self.banner(format!("Failed to complete setup: {err}"));
                return;
            }
        };
        if let Some(extras) = response.updated.remove(SINGLETON_ID) {
            let extras = match extras {
                Some(Value::Object(extras)) => extras,
                _ => Map::new(),
            };
            self.phase = Phase::Complete(self.extras(&cx.conn.schema, extras));
            return;
        }
        match response
            .not_updated
            .iter()
            .find(|(id, _)| *id == SINGLETON_ID)
        {
            Some((_, err)) => self.rejected(err),
            None => self.banner(NO_CONFIRM.into()),
        }
    }

    fn rejected(&mut self, err: &SetError) {
        let Some(form) = &mut self.form else {
            return;
        };
        form.collapse();
        form.model.clear_errors();
        form.model.layout = self.full.clone();
        form.model
            .apply_set_error(err, &set_error_plain(err, &RefLabels::default()));
        let page = self
            .pages
            .iter()
            .position(|page| {
                page.rows
                    .iter()
                    .any(|row| form.model.error(&row.name).is_some())
            })
            .unwrap_or(self.page);
        self.show(page);
    }

    fn banner(&mut self, text: String) {
        if let Some(form) = &mut self.form {
            form.model.banner = Some(text.into_boxed_str());
        }
    }

    fn extras(&self, schema: &Schema, extras: Map<String, Value>) -> Vec<Extra> {
        let fields = self
            .form
            .as_ref()
            .and_then(|form| form.model.fields(schema));
        extras
            .into_iter()
            .filter(|(key, _)| !HIDDEN_EXTRAS.contains(&key.as_str()))
            .filter_map(|(key, value)| match value {
                Value::String(value) if !value.is_empty() => Some((key, value)),
                _ => None,
            })
            .map(|(key, value)| {
                let label = self
                    .full
                    .rows
                    .iter()
                    .find(|row| *row.name == *key)
                    .map(|row| row.label.clone())
                    .or_else(|| {
                        fields
                            .and_then(|fields| fields.properties.get(&key))
                            .and_then(|field| field.description.lines().next())
                            .filter(|line| !line.is_empty())
                            .map(Box::from)
                    })
                    .unwrap_or_else(|| key.into_boxed_str());
                Extra {
                    label,
                    value: value.into_boxed_str(),
                }
            })
            .collect()
    }

    fn pick(&mut self, cx: &mut Cx<'_>, pick: Pick) {
        let Some(root) = self.form.as_ref() else {
            return;
        };
        let form = root.active();
        let Some((label, current)) = root.pick_target(&pick) else {
            return;
        };
        let schema = &cx.conn.schema;
        let picker = match &pick.kind {
            PickKind::Variant => {
                let items = form
                    .model
                    .shape
                    .variants(schema)
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
                let mut items = enum_items(schema, name);
                if *nullable && !*multi {
                    items.insert(0, PickerItem::new("", "None", "clear the value"));
                }
                let selection = if *multi {
                    Selection::Multi
                } else {
                    Selection::Single
                };
                let picker = Picker::new(label, items, selection);
                match current {
                    Some(Value::Object(set)) => {
                        picker.with_marked(set.keys().map(|key| (key.as_str(), key.as_str())))
                    }
                    value => picker.with_current(value.and_then(Value::as_str)),
                }
            }
            _ => {
                cx.flash
                    .set(Level::Warn, "This field cannot be chosen here", cx.now);
                return;
            }
        };
        self.layers.push(Layer::Picker {
            picker,
            purpose: Purpose::FieldPick(Box::new(pick)),
        });
    }

    fn layer_event(&mut self, cx: &mut Cx<'_>, event: LayerEvent) {
        if let LayerEvent::Chosen {
            purpose: Purpose::FieldPick(pick),
            items,
        } = event
            && let Some(form) = &mut self.form
        {
            let keys: Vec<Box<str>> = items.into_iter().map(|(key, _)| key).collect();
            form.apply_pick(&cx.conn.schema, &every_object, &pick, &keys);
        }
    }

    fn sign_in(cx: &mut Cx<'_>) {
        let dialog = Dialog::confirm(
            "Sign in again",
            Risk::Low,
            vec![Line::from(
                "Return to the login screen? Restart Stalwart first so the new configuration \
                 takes effect.",
            )],
            CONFIRM_LOGOUT,
        );
        cx.overlays
            .push(Overlay::Confirm(Box::new(dialog), Confirmable::Logout));
    }
}

impl Default for State {
    fn default() -> Self {
        State::new()
    }
}

fn every_object(_: &str) -> bool {
    true
}

fn pages(layout: &Layout) -> Vec<Layout> {
    layout
        .sections
        .iter()
        .filter_map(|section| {
            let rows = layout.rows.get(section.rows.clone())?.to_vec();
            Some(Layout {
                title: section.title.clone(),
                sections: vec![Section {
                    title: section.title.clone(),
                    rows: 0..rows.len(),
                }],
                rows,
            })
        })
        .collect()
}

fn merge(into: &mut Map<String, Value>, from: Map<String, Value>) {
    for (key, value) in from {
        match (into.get_mut(&key), value) {
            (Some(Value::Object(target)), Value::Object(source)) => merge(target, source),
            (_, value) => {
                into.insert(key, value);
            }
        }
    }
}

pub fn is_bootstrap_mode(schema: &Schema, permissions: &Permissions, links: Links) -> bool {
    permissions.is_bootstrap()
        || (links == Links::None && allowed(schema, permissions, VIEW, Op::Get))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Links {
    None,
    Visible,
}

pub fn context(state: &State) -> Context {
    match state.layers.context() {
        Some(context) => context,
        None if state.phase == Phase::Editing
            && let Some(context) = state
                .form
                .as_ref()
                .filter(|form| form.is_editing())
                .and_then(FormView::edit_context) =>
        {
            context
        }
        None if matches!(state.phase, Phase::Complete(_)) => Context::BootstrapComplete,
        None => Context::Bootstrap,
    }
}

pub fn handle_key(state: &mut State, cx: &mut Cx<'_>, event: KeyEvent) -> bool {
    if !state.layers.is_empty() {
        let event = state.layers.handle(event);
        state.layer_event(cx, event);
        return true;
    }
    if state.phase == Phase::Editing
        && let Some(form) = &mut state.form
        && form.is_editing()
    {
        let action = keymap::resolve(EDIT_KEYS, event);
        form.edit_key(&cx.conn.schema, &every_object, event, action);
        return true;
    }
    let keys = match state.phase {
        Phase::Complete(_) => COMPLETE_KEYS,
        _ => KEYS,
    };
    match keymap::resolve(keys, event) {
        Some(Action::Back)
            if state
                .form
                .as_ref()
                .is_none_or(|form| form.frames().is_empty()) =>
        {
            false
        }
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
        ScreenMsg::Paste(text) if state.layers.is_empty() => {
            if let Some(form) = &mut state.form {
                form.paste(text);
            }
        }
        ScreenMsg::Paste(text) => state.layers.paste(text),
        ScreenMsg::Done(JobOutput::Objects(output)) => match output {
            ObjectsOutput::Get(result) => state.loaded(cx, result),
            ObjectsOutput::Set(result) => state.saved(cx, result),
            _ => {}
        },
        ScreenMsg::Text(_)
        | ScreenMsg::Done(_)
        | ScreenMsg::Stream(_)
        | ScreenMsg::Tick(_)
        | ScreenMsg::Resumed(_)
        | ScreenMsg::Progress(_) => {}
    }
}

fn act(state: &mut State, cx: &mut Cx<'_>, action: Action) {
    match (&state.phase, action) {
        (Phase::Failed(_), Action::Retry) => state.start(cx),
        (Phase::Complete(extras), Action::Down) => {
            state.cursor = (state.cursor + 1).min(extras.len().saturating_sub(1));
        }
        (Phase::Complete(_), Action::Up) => state.cursor = state.cursor.saturating_sub(1),
        (Phase::Complete(extras), Action::Copy) => {
            if let Some(extra) = extras.get(state.cursor) {
                cx.fx.push(Effect::Copy(extra.value.clone()));
                cx.flash.set(Level::Ok, "Copied", cx.now);
            }
        }
        (Phase::Complete(_), Action::SignIn) => State::sign_in(cx),
        (Phase::Editing, Action::Next) => state.next(cx),
        (Phase::Editing, Action::Prev) => state.prev(),
        (Phase::Editing, Action::Finish) => state.finish(cx),
        (Phase::Editing, action) => {
            let form_action = match action {
                Action::Down => FormAction::Down,
                Action::Up => FormAction::Up,
                Action::Edit => FormAction::Edit,
                Action::Toggle => FormAction::Toggle,
                Action::Revert => FormAction::Revert,
                Action::Clear => FormAction::Clear,
                Action::Add => FormAction::Add,
                Action::MoveUp => FormAction::MoveUp,
                Action::MoveDown => FormAction::MoveDown,
                Action::Back => FormAction::Back,
                _ => return,
            };
            let Some(form) = &mut state.form else {
                return;
            };
            if let FormEvent::Pick(pick) = form.act(&cx.conn.schema, &every_object, form_action) {
                state.pick(cx, pick);
            }
        }
        _ => {}
    }
}

pub fn render(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    let focused = cx.focused && state.layers.is_empty();
    let lines: Vec<Line<'_>> = match &state.phase {
        Phase::Editing | Phase::Saving => {
            if let Some(form) = &mut state.form {
                let step = format!(
                    "{TITLE} · Step {} of {}{}",
                    state.page + 1,
                    state.pages.len(),
                    if state.phase == Phase::Saving {
                        " · saving…"
                    } else {
                        ""
                    }
                );
                render::render(
                    form,
                    frame,
                    area,
                    &FormFrame {
                        title: &step,
                        schema: &cx.conn.schema,
                        labels: &cx.conn.labels,
                        theme,
                        focused,
                    },
                );
            }
            state.layers.render(frame, area, theme);
            return;
        }
        Phase::Loading => vec![Line::styled("Loading setup…", theme.muted())],
        Phase::Failed(error) => vec![
            Line::styled(&**error, theme.text().fg(theme.err)),
            Line::raw(""),
            Line::styled("r retry", theme.muted()),
        ],
        Phase::Empty => vec![Line::styled(
            "Setup form is empty. The server did not return any bootstrap fields.",
            theme.text().fg(theme.err),
        )],
        Phase::Complete(extras) => {
            let mut lines = vec![
                Line::styled("Setup complete", theme.title()),
                Line::raw(""),
                Line::raw(if extras.is_empty() {
                    COMPLETE_WITHOUT_CREDENTIALS
                } else {
                    COMPLETE_WITH_CREDENTIALS
                }),
                Line::raw(""),
            ];
            for (index, extra) in extras.iter().enumerate() {
                let marker = if index == state.cursor { "▌ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(marker, theme.selected),
                    Span::styled(format!("{}: ", extra.label), theme.title()),
                    Span::styled(&*extra.value, theme.text()),
                ]));
            }
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled("Next step: ", theme.title()),
                Span::raw(if extras.is_empty() {
                    NEXT_WITHOUT_CREDENTIALS
                } else {
                    NEXT_WITH_CREDENTIALS
                }),
            ]));
            lines
        }
    };
    let block = theme.panel(Line::from(TITLE), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    state.layers.render(frame, area, theme);
}

pub fn crumbs(state: &State, out: &mut Crumbs) {
    out.push("Setup");
    if let Some(title) = state
        .pages
        .get(state.page)
        .and_then(|page| page.title.as_deref())
        && matches!(state.phase, Phase::Editing | Phase::Saving)
    {
        out.push(title);
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/bootstrap/tests.rs"]
mod tests;
