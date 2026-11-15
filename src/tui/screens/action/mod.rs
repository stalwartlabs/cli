/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::account::Op;
use crate::jmap::response::SetResponse;
use crate::schema::AT_TYPE;
use crate::tui::app::{Cx, ScreenMsg, ViewCx};
use crate::tui::exec::Slot;
use crate::tui::form::layer::{Layer, LayerEvent, Layers, Purpose};
use crate::tui::form::model::allowed;
use crate::tui::job::objects::{ObjectsKind, ObjectsOutput, ObjectsSlot, SetSpec};
use crate::tui::keymap::{self, Context, code, key, keys};
use crate::tui::msg::{Effect, JobOutput};
use crate::tui::screens::form::{self, Target, save};
use crate::tui::screens::objects_job;
use crate::tui::session::Connected;
use crate::tui::theme::Level;
use crate::tui::widgets::chrome::Crumbs;
use crate::tui::widgets::dialog::{CONFIRM_RUN, Dialog, Risk};
use crate::view::RefLabels;
use crate::view::set_error_plain;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::{Map, Value, json};
use std::time::Instant;

const PERMISSION_PREFIX: &str = "action";
const DEFAULT_GROUP: &str = "General";
const CREATE_ID: &str = "new";
const TITLE: &str = "Actions";
const NOT_PERMITTED: &str = "Running actions needs the Create permission on actions";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Run,
    Back,
}

keys! { pub KEYS, pub HINTS: Action;
    "j/k" Down "move" true [key('j'), code(KeyCode::Down)],
    "k" Up "up" false [key('k'), code(KeyCode::Up)],
    "⏎" Run "run" true [code(KeyCode::Enter)],
    "esc" Back "back" false [code(KeyCode::Esc), code(KeyCode::Backspace)],
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    variant: Box<str>,
    group: usize,
    label: Box<str>,
}

pub struct State {
    view: Box<str>,
    object: Box<str>,
    groups: Vec<Box<str>>,
    entries: Vec<Entry>,
    cursor: usize,
    scroll: usize,
    layers: Layers,
    form: Option<Box<form::State>>,
    sent: Option<Box<str>>,
    can_create: bool,
}

impl State {
    pub fn new(conn: &Connected, view: &str) -> Self {
        let mut groups: Vec<Box<str>> = Vec::new();
        let mut entries = Vec::new();
        let mut permission = String::new();
        let def = conn.schema.view(view);
        let object = def.map_or(view, |def| def.object.name());
        let variants = def.map_or(&[][..], |def| def.object.variants());
        for variant in variants {
            permission.clear();
            permission.push_str(PERMISSION_PREFIX);
            permission.push_str(&variant.name);
            if !conn.info.permissions.has(&permission) {
                continue;
            }
            let label = if variant.label.is_empty() {
                &variant.name
            } else {
                &variant.label
            };
            let (group, label) = label
                .split_once(':')
                .map_or((DEFAULT_GROUP, label.as_str()), |(group, label)| {
                    (group.trim(), label.trim())
                });
            let group = match groups.iter().position(|known| **known == *group) {
                Some(index) => index,
                None => {
                    groups.push(group.into());
                    groups.len() - 1
                }
            };
            entries.push(Entry {
                variant: variant.name.as_str().into(),
                group,
                label: label.into(),
            });
        }
        entries.sort_by_key(|entry| entry.group);
        State {
            view: view.into(),
            object: object.into(),
            groups,
            entries,
            cursor: 0,
            scroll: 0,
            layers: Layers::default(),
            form: None,
            sent: None,
            can_create: allowed(&conn.schema, &conn.info.permissions, view, Op::Create),
        }
    }

    fn run(&mut self, cx: &mut Cx<'_>) {
        let Some(entry) = self.entries.get(self.cursor) else {
            return;
        };
        if !self.can_create {
            cx.flash.set(Level::Warn, NOT_PERMITTED, cx.now);
            return;
        }
        let form = form::State::new(cx.conn, &self.view, Target::Action(entry.variant.clone()));
        if form.has_fields() {
            self.form = Some(Box::new(form));
            return;
        }
        let args = json!({ "create": { CREATE_ID: { "@type": &*entry.variant } } });
        let body = vec![
            Line::raw(format!("Run {}?", entry.label)),
            Line::raw(""),
            Line::raw(format!("{}/set {args}", self.object)),
        ];
        self.layers.push(Layer::Dialog {
            dialog: Dialog::confirm(&*entry.label, Risk::Low, body, CONFIRM_RUN),
            purpose: Purpose::Apply(Box::new(args)),
        });
    }

    fn send(&self, cx: &mut Cx<'_>, args: Value) {
        let spec = SetSpec {
            object: self.object.clone(),
            args,
            uploads: Vec::new(),
        };
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Save));
        cx.fx
            .spawn(ticket, objects_job(cx, ObjectsKind::Set(Box::new(spec))));
    }

    fn ran(&mut self, cx: &mut Cx<'_>, response: &SetResponse) {
        cx.conn.pickers.clear();
        if let Some((_, err)) = response.not_created.first() {
            let text = set_error_plain(err, &RefLabels::default());
            cx.flash.set(Level::Error, text, cx.now);
            return;
        }
        let created = response.created.get(CREATE_ID);
        let result = save::action_result(&Map::new(), created, self.sent.as_deref());
        form::show_result(&mut self.layers, &self.view, result, created);
    }

    fn layer_event(&mut self, cx: &mut Cx<'_>, event: LayerEvent) {
        match event {
            LayerEvent::Confirmed(Purpose::Apply(args)) => {
                self.sent = args
                    .get("create")
                    .and_then(|create| create.get(CREATE_ID)?.get(AT_TYPE)?.as_str())
                    .map(Box::from);
                self.send(cx, *args);
            }
            LayerEvent::Confirmed(Purpose::Created(text)) if !text.is_empty() => {
                cx.fx.push(Effect::Copy(text))
            }
            LayerEvent::Pending
            | LayerEvent::Closed(_)
            | LayerEvent::Confirmed(_)
            | LayerEvent::Chosen { .. } => {}
        }
    }

    fn close_form(&mut self) {
        if self.form.as_ref().is_some_and(|form| form.closed()) {
            self.form = None;
        }
    }
}

pub fn context(state: &State) -> Context {
    match (&state.form, state.layers.context()) {
        (Some(form), _) => form::context(form),
        (None, Some(ctx)) => ctx,
        (None, None) => Context::Action,
    }
}

pub fn deadline(state: &State) -> Option<Instant> {
    state.form.as_deref().and_then(form::deadline)
}

pub fn handle_key(state: &mut State, cx: &mut Cx<'_>, event: KeyEvent) -> bool {
    if let Some(form) = &mut state.form {
        let handled = form::handle_key(form, cx, event);
        state.close_form();
        return handled;
    }
    if !state.layers.is_empty() {
        let event = state.layers.handle(event);
        state.layer_event(cx, event);
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
    if let Some(form) = &mut state.form {
        let msg = match msg {
            ScreenMsg::Key(_) | ScreenMsg::Text(_) => return,
            ScreenMsg::Paste(text) => ScreenMsg::Paste(text),
            ScreenMsg::Done(output) => ScreenMsg::Done(output),
            ScreenMsg::Stream(event) => ScreenMsg::Stream(event),
            ScreenMsg::Tick(now) => ScreenMsg::Tick(now),
            ScreenMsg::Resumed(error) => ScreenMsg::Resumed(error),
            ScreenMsg::Progress(progress) => ScreenMsg::Progress(progress),
        };
        form::update(form, cx, msg);
        state.close_form();
        return;
    }
    match msg {
        ScreenMsg::Key(action) => act(state, cx, action),
        ScreenMsg::Paste(text) => state.layers.paste(text),
        ScreenMsg::Done(JobOutput::Objects(ObjectsOutput::Set(result))) => match result {
            Ok(response) => state.ran(cx, &response),
            Err(err) => cx.flash.set(Level::Error, err.to_string(), cx.now),
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
    match action {
        Action::Up => state.cursor = state.cursor.saturating_sub(1),
        Action::Down => {
            state.cursor = (state.cursor + 1).min(state.entries.len().saturating_sub(1))
        }
        Action::Run => state.run(cx),
        Action::Back => cx.back(),
    }
}

pub fn render(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    if let Some(form) = &mut state.form {
        form::render(form, frame, area, cx);
        return;
    }
    let theme = cx.theme;
    let block = theme.panel(Line::from(TITLE), cx.focused && state.layers.is_empty());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = Vec::with_capacity(state.entries.len() + state.groups.len() * 2);
    let mut cursor_line = 0;
    let mut group = None;
    if !state.can_create && !state.entries.is_empty() {
        lines.push(Line::styled(NOT_PERMITTED, theme.muted()));
        lines.push(Line::raw(""));
    }
    for (index, entry) in state.entries.iter().enumerate() {
        if group != Some(entry.group) {
            if group.is_some() {
                lines.push(Line::raw(""));
            }
            group = Some(entry.group);
            let title = state.groups.get(entry.group).map_or("", AsRef::as_ref);
            lines.push(Line::styled(title, theme.title()));
        }
        if index == state.cursor && state.can_create {
            cursor_line = lines.len();
            lines.push(Line::from(vec![
                Span::styled("▌ ", theme.selected),
                Span::styled(&*entry.label, theme.selected),
            ]));
        } else {
            lines.push(Line::from(vec![Span::raw("  "), Span::raw(&*entry.label)]));
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "No actions are available to this account.",
            theme.muted(),
        ));
    }
    let height = usize::from(inner.height).max(1);
    if cursor_line < state.scroll {
        state.scroll = cursor_line.saturating_sub(1);
    } else if cursor_line >= state.scroll + height {
        state.scroll = cursor_line + 1 - height;
    }
    let scroll = u16::try_from(state.scroll).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
    state.layers.build_docs(cx);
    state.layers.render(frame, area, theme);
}

pub fn crumbs(state: &State, out: &mut Crumbs) {
    out.push(TITLE);
    if let Some(form) = &state.form {
        form::crumbs(form, out);
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/action/tests.rs"]
mod tests;
