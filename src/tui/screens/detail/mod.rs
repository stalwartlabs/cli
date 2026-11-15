/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod doc;

use crate::schema::FieldType;
use crate::tui::app::{Cx, ScreenMsg, ViewCx};
use crate::tui::exec::Slot;
use crate::tui::job::objects::{
    BlobSpec, BlobTexts, GetSpec, ObjectsKind, ObjectsOutput, ObjectsSlot, Reply,
};
use crate::tui::keymap::{self, Context, code, ctrl, key, keys, upper};
use crate::tui::msg::{Effect, JobOutput};
use crate::tui::screens::objects_job;
use crate::tui::session::Connected;
use crate::tui::theme::Level;
use crate::tui::widgets::chrome::Crumbs;
use crate::view::ObjectDoc;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use serde_json::{Map, Value};
use std::slice;

const PAGE: u16 = 10;
const BORDERS: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    PageDown,
    PageUp,
    Copy,
    Reload,
    Back,
}

keys! { pub KEYS, pub HINTS: Action;
    "j/k" Down "scroll" true [key('j'), code(KeyCode::Down)],
    "k" Up "scroll up" false [key('k'), code(KeyCode::Up)],
    "pgdn" PageDown "page down" false [code(KeyCode::PageDown), ctrl('f')],
    "pgup" PageUp "page up" false [code(KeyCode::PageUp), ctrl('b')],
    "y" Copy "copy JSON" true [key('y')],
    "R" Reload "reload" true [upper('R')],
    "esc" Back "back" true [code(KeyCode::Esc), code(KeyCode::Backspace)],
}

pub struct State {
    object: Box<str>,
    id: Box<str>,
    heading: Option<Box<str>>,
    title: Box<str>,
    value: Option<Value>,
    lines: Vec<Line<'static>>,
    lines_width: u16,
    blobs: BlobTexts,
    status: Box<str>,
    scroll: u16,
}

impl State {
    pub fn new(conn: &Connected, view: &str, id: &str) -> Self {
        let def = conn.schema.view(view);
        let list = def.as_ref().and_then(|def| def.list);
        State {
            object: def.as_ref().map_or(view, |def| def.object.name()).into(),
            id: id.into(),
            heading: list.map(|list| list.title.as_str().into()),
            title: list.map_or_else(|| id.into(), |list| capitalized(&list.singular_name)),
            value: None,
            lines: Vec::new(),
            lines_width: 0,
            blobs: Vec::new(),
            status: "loading…".into(),
            scroll: 0,
        }
    }

    pub fn start(&mut self, cx: &mut Cx<'_>) {
        let spec = GetSpec {
            object: self.object.clone(),
            id: self.id.clone(),
            properties: None,
        };
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Load));
        cx.fx
            .spawn(ticket, objects_job(cx, ObjectsKind::Get(Box::new(spec))));
    }

    fn loaded(&mut self, cx: &mut Cx<'_>, result: Reply<Option<Map<String, Value>>>) {
        let object = match result.map(|object| *object) {
            Ok(Some(object)) => object,
            Ok(None) => {
                self.status = "This object no longer exists.".into();
                return;
            }
            Err(err) => {
                self.status = err.to_string().into();
                return;
            }
        };
        let schema = cx.conn.schema.clone();
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
        let blobs: Vec<(Box<str>, Box<str>)> =
            match (&cx.conn.active_account, schema.get_object(&self.object)) {
                (Some(_), Some(def)) => def
                    .all_fields()
                    .all_defs()
                    .filter(|(_, field)| matches!(field.typ, FieldType::BlobId))
                    .filter_map(|(name, _)| Some((name.into(), object.get(name)?.as_str()?.into())))
                    .collect(),
                _ => Vec::new(),
            };
        if !blobs.is_empty() {
            let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Load));
            let kind = ObjectsKind::BlobTexts(Box::new(BlobSpec { blobs }));
            cx.fx.spawn(ticket, objects_job(cx, kind));
        }
        self.value = Some(Value::Object(object));
        self.blobs.clear();
        self.lines.clear();
    }

    fn blobs_loaded(&mut self, cx: &mut Cx<'_>, result: Reply<BlobTexts>) {
        match result {
            Ok(texts) => {
                self.blobs = *texts;
                self.lines.clear();
            }
            Err(err) => cx.flash.set(Level::Error, err.to_string(), cx.now),
        }
    }
}

pub fn context(_state: &State) -> Context {
    Context::Detail
}

pub fn handle_key(state: &mut State, cx: &mut Cx<'_>, event: KeyEvent) -> bool {
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
        ScreenMsg::Done(JobOutput::Objects(ObjectsOutput::Get(result))) => state.loaded(cx, result),
        ScreenMsg::Done(JobOutput::Objects(ObjectsOutput::BlobTexts(result))) => {
            state.blobs_loaded(cx, result)
        }
        ScreenMsg::Done(JobOutput::Objects(ObjectsOutput::Labels { labels, .. })) => {
            cx.conn.labels.insert_all(*labels);
            state.lines.clear();
        }
        ScreenMsg::Text(_)
        | ScreenMsg::Paste(_)
        | ScreenMsg::Done(_)
        | ScreenMsg::Stream(_)
        | ScreenMsg::Tick(_)
        | ScreenMsg::Resumed(_)
        | ScreenMsg::Progress(_) => {}
    }
}

fn act(state: &mut State, cx: &mut Cx<'_>, action: Action) {
    match action {
        Action::Down => state.scroll = state.scroll.saturating_add(1),
        Action::Up => state.scroll = state.scroll.saturating_sub(1),
        Action::PageDown => state.scroll = state.scroll.saturating_add(PAGE),
        Action::PageUp => state.scroll = state.scroll.saturating_sub(PAGE),
        Action::Copy => match state
            .value
            .as_ref()
            .and_then(|value| serde_json::to_string_pretty(value).ok())
        {
            Some(text) => {
                cx.fx.push(Effect::Copy(text.into()));
                cx.flash.set(Level::Ok, "Copied", cx.now);
            }
            None => cx.flash.set(Level::Warn, "Nothing to copy yet", cx.now),
        },
        Action::Reload => {
            state.value = None;
            state.lines.clear();
            state.status = "loading…".into();
            state.start(cx);
        }
        Action::Back => cx.back(),
    }
}

pub fn render(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    let schema = &cx.conn.schema;
    let width = area.width.saturating_sub(BORDERS);
    if width != state.lines_width {
        state.lines.clear();
        state.lines_width = width;
    }
    if state.lines.is_empty()
        && let (Some(value), Some(def)) = (&state.value, schema.get_object(&state.object))
    {
        let doc = ObjectDoc::build(schema, &cx.conn.labels, def, value);
        if let Some(label) = doc.label {
            state.title = label.into();
        }
        doc::lines(&doc, theme, usize::from(width), &mut state.lines);
        for (name, text) in &state.blobs {
            let Some(text) = text else {
                continue;
            };
            state.lines.push(Line::raw(""));
            state
                .lines
                .push(Line::styled(format!("── {name} ──"), theme.title()));
            state
                .lines
                .extend(text.lines().map(|line| Line::raw(line.to_owned())));
        }
    }
    let block = theme.panel(Line::from_iter([" ", &*state.title, " "]), cx.focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if state.value.is_none() {
        frame.render_widget(Line::styled(&*state.status, theme.muted()), inner);
        return;
    }
    frame.render_widget(
        Paragraph::new(state.lines.as_slice())
            .wrap(Wrap { trim: false })
            .scroll((state.scroll, 0)),
        inner,
    );
}

fn capitalized(text: &str) -> Box<str> {
    let mut chars = text.chars();
    let mut out = String::with_capacity(text.len());
    if let Some(first) = chars.next() {
        out.extend(first.to_uppercase());
        out.push_str(chars.as_str());
    }
    out.into()
}

pub fn crumbs(state: &State, out: &mut Crumbs) {
    if let Some(heading) = &state.heading {
        out.push(heading);
    }
    out.push(&state.title);
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/detail/tests.rs"]
mod tests;
