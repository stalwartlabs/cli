/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Error;
use crate::jmap::live::TraceEvent;
use crate::tui::app::{Cx, ScreenMsg};
use crate::tui::exec::Slot;
use crate::tui::keymap::{self, Context, code, key, keys, upper};
use crate::tui::msg::{Effect, StreamEvent, StreamId, StreamSpec};
use crate::tui::session::Connected;
use crate::tui::widgets::chrome::Crumbs;
use crate::tui::widgets::input::Input;
use crate::tui::widgets::picker::{self, Picker, PickerItem, Selection};
use crate::tui::widgets::timeline::{Labels, Timeline};
use crate::view::time::UtcTimestamp;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::collections::VecDeque;

mod view;

pub use view::render;

pub const MAX_EVENTS: usize = 1000;
const RESERVED_KEYS: [&str; 2] = ["filter", "token"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Edit,
    Leave,
    Focus,
    Add,
    Remove,
    Prev,
    Next,
    Confirm,
    Cancel,
    Stop,
    NewTrace,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

keys! { pub IDLE_KEYS, pub IDLE_HINTS: Action;
    "⏎" Start "start tracing" true [code(KeyCode::Enter)],
    "e" Edit "edit keywords" true [key('e'), key('i')],
    "tab" Focus "edit filters" true [code(KeyCode::Tab)],
}

keys! { pub INPUT_KEYS, pub INPUT_HINTS: Action;
    "⏎" Start "start tracing" true [code(KeyCode::Enter)],
    "tab" Focus "edit filters" true [code(KeyCode::Tab)],
    "esc" Leave "leave the field" true [code(KeyCode::Esc)],
}

keys! { pub FILTER_KEYS, pub FILTER_HINTS: Action;
    "a" Add "add filter" true [key('a')],
    "x" Remove "remove filter" true [key('x'), code(KeyCode::Delete), code(KeyCode::Backspace)],
    "←→" Prev "select filter" true [code(KeyCode::Left), key('h')],
    "→" Next "next filter" false [code(KeyCode::Right), key('l')],
    "⏎" Start "start tracing" true [code(KeyCode::Enter)],
    "tab" Focus "edit keywords" true [code(KeyCode::Tab)],
    "esc" Leave "done with filters" true [code(KeyCode::Esc)],
}

keys! { pub VALUE_KEYS, pub VALUE_HINTS: Action;
    "⏎" Confirm "add filter" true [code(KeyCode::Enter)],
    "esc" Cancel "cancel" true [code(KeyCode::Esc)],
}

keys! { pub SESSION_KEYS, pub SESSION_HINTS: Action;
    "s" Stop "stop" true [key('s')],
    "n" NewTrace "new trace" true [key('n')],
    "↑↓" Up "scroll" true [code(KeyCode::Up), key('k')],
    "↓" Down "scroll down" false [code(KeyCode::Down), key('j')],
    "pgup" PageUp "page up" false [code(KeyCode::PageUp)],
    "pgdn" PageDown "page down" false [code(KeyCode::PageDown)],
    "g" Top "first event" false [key('g'), code(KeyCode::Home)],
    "G" Bottom "follow newest" true [upper('G'), code(KeyCode::End)],
}

pub struct Filter {
    pub key: Box<str>,
    pub label: Box<str>,
    pub value: Box<str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormFocus {
    Idle,
    Keywords,
    Filters(usize),
}

pub enum Adding {
    Idle,
    Picking(Box<Picker>),
    Value { key: Box<str>, input: Input },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Streaming,
    Reconnecting(u8),
    Stopped,
    Failed(Box<str>),
}

impl Status {
    pub fn active(&self) -> bool {
        matches!(
            self,
            Status::Connecting | Status::Streaming | Status::Reconnecting(_)
        )
    }
}

pub struct Run {
    stream: StreamId,
    pub status: Status,
    opened: bool,
    pub events: VecDeque<TraceEvent>,
    pub anchor: Option<UtcTimestamp>,
    pub timeline: Timeline,
    pub summary: Box<str>,
}

pub struct State {
    pub labels: Labels,
    pub keywords: Input,
    pub filters: Vec<Filter>,
    pub focus: FormFocus,
    pub adding: Adding,
    pub run: Option<Run>,
}

impl State {
    pub fn new(conn: &Connected) -> Self {
        State {
            labels: Labels::new(conn.schema.clone()),
            keywords: Input::default(),
            filters: Vec::new(),
            focus: FormFocus::Idle,
            adding: Adding::Idle,
            run: None,
        }
    }

    fn start(&mut self, cx: &mut Cx<'_>) {
        let stream = StreamId::start(cx.slots);
        let keywords = self.keywords.value().trim();
        let filter = (!keywords.is_empty()).then(|| Box::from(keywords));
        let pairs = self
            .filters
            .iter()
            .map(|f| (f.key.clone(), f.value.clone()))
            .collect();
        let mut summary = String::new();
        if let Some(filter) = &filter {
            summary.push('"');
            summary.push_str(filter);
            summary.push('"');
        }
        for filter in &self.filters {
            if !summary.is_empty() {
                summary.push_str(" · ");
            }
            summary.push_str(&filter.label);
            summary.push_str(" = ");
            summary.push_str(&filter.value);
        }
        cx.fx.push(Effect::OpenStream(
            stream,
            StreamSpec::Tracing { filter, pairs },
        ));
        self.run = Some(Run {
            stream,
            status: Status::Connecting,
            opened: false,
            events: VecDeque::with_capacity(MAX_EVENTS),
            anchor: None,
            timeline: Timeline::new(true),
            summary: summary.into(),
        });
    }

    fn stop(&mut self, cx: &mut Cx<'_>) {
        if let Some(run) = &mut self.run
            && run.status.active()
        {
            cx.fx.push(Effect::CloseStream(run.stream));
            cx.slots.bump(&Slot::Stream);
            run.status = Status::Stopped;
        }
    }

    fn open_picker(&mut self) {
        let items = self
            .labels
            .key_variants()
            .iter()
            .filter(|v| !RESERVED_KEYS.contains(&v.name.as_str()))
            .map(|v| {
                let label = if v.label.is_empty() {
                    &v.name
                } else {
                    &v.label
                };
                PickerItem::new(
                    v.name.as_str(),
                    label.as_str(),
                    v.explanation.as_deref().unwrap_or_default(),
                )
            })
            .collect();
        self.adding = Adding::Picking(Box::new(Picker::new(
            "Filter key",
            items,
            Selection::Single,
        )));
    }

    fn add_filter(&mut self, key: Box<str>, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        let label = Box::from(self.labels.key_label(&key));
        let filter = Filter {
            key,
            label,
            value: value.into(),
        };
        match self.filters.iter_mut().find(|f| f.key == filter.key) {
            Some(existing) => *existing = filter,
            None => self.filters.push(filter),
        }
    }

    fn push_events(&mut self, events: Vec<TraceEvent>) {
        let Some(run) = &mut self.run else {
            return;
        };
        if run.anchor.is_none() {
            run.anchor = events.first().map(|event| event.timestamp);
        }
        let overflow = (run.events.len() + events.len()).saturating_sub(MAX_EVENTS);
        let drop_old = overflow.min(run.events.len());
        run.events.drain(..drop_old);
        let skip = events.len().saturating_sub(MAX_EVENTS);
        run.events.extend(events.into_iter().skip(skip));
        run.timeline.dropped(overflow);
    }

    fn stream_event(&mut self, event: StreamEvent) {
        if let StreamEvent::Trace(events) = event {
            self.push_events(events);
            return;
        }
        let Some(run) = &mut self.run else {
            return;
        };
        match event {
            StreamEvent::Open => {
                run.opened = true;
                run.status = Status::Streaming;
            }
            StreamEvent::Reconnecting { attempt, .. } => run.status = Status::Reconnecting(attempt),
            StreamEvent::Closed(None) => run.status = Status::Stopped,
            StreamEvent::Closed(Some(error)) => {
                run.status = Status::Failed(failure(&error, run.opened))
            }
            StreamEvent::Metrics(_) | StreamEvent::Stage(_) | StreamEvent::Trace(_) => {}
        }
    }
}

fn failure(error: &Error, opened: bool) -> Box<str> {
    match error {
        Error::PermissionDenied => "You do not have permission to perform this action.".into(),
        Error::NotFound => {
            "Live tracing is not available on this server (Enterprise feature).".into()
        }
        _ if opened => {
            format!("Live tracing stream disconnected after multiple retries: {error}").into()
        }
        _ => format!("The server refused the live tracing stream: {error}").into(),
    }
}

pub fn context(state: &State) -> Context {
    match (&state.run, &state.adding, state.focus) {
        (Some(_), ..) => Context::Tracing,
        (None, Adding::Picking(_), _) => Context::Picker,
        (None, Adding::Value { .. }, _) => Context::TracingValue,
        (None, Adding::Idle, FormFocus::Idle) => Context::TracingForm,
        (None, Adding::Idle, FormFocus::Keywords) => Context::TracingInput,
        (None, Adding::Idle, FormFocus::Filters(_)) => Context::TracingFilters,
    }
}

pub fn available(state: &State, action: Action) -> bool {
    match action {
        Action::Stop => state.run.as_ref().is_some_and(|run| run.status.active()),
        _ => true,
    }
}

pub fn handle_key(state: &mut State, cx: &mut Cx<'_>, event: KeyEvent) -> bool {
    if state.run.is_some() {
        return match keymap::resolve(SESSION_KEYS, event) {
            Some(action) => {
                act(state, cx, action);
                true
            }
            None => false,
        };
    }
    match &mut state.adding {
        Adding::Picking(picker) => {
            match picker.handle(event) {
                picker::Outcome::Chosen(index) => {
                    let key = picker.item(index).map(|item| item.key.clone());
                    state.adding = match key {
                        Some(key) => Adding::Value {
                            key,
                            input: Input::default(),
                        },
                        None => Adding::Idle,
                    };
                }
                picker::Outcome::Cancel => state.adding = Adding::Idle,
                picker::Outcome::Pending
                | picker::Outcome::QueryChanged
                | picker::Outcome::ChosenMany(_) => {}
            }
            return true;
        }
        Adding::Value { input, .. } => {
            match keymap::resolve(VALUE_KEYS, event) {
                Some(action) => act(state, cx, action),
                None => {
                    input.handle(event);
                }
            }
            return true;
        }
        Adding::Idle => {}
    }
    match state.focus {
        FormFocus::Idle => match keymap::resolve(IDLE_KEYS, event) {
            Some(action) => {
                act(state, cx, action);
                true
            }
            None => false,
        },
        FormFocus::Keywords => match keymap::resolve(INPUT_KEYS, event) {
            Some(action) => {
                act(state, cx, action);
                true
            }
            None if keymap::is_text(event) => state.keywords.handle(event),
            None => false,
        },
        FormFocus::Filters(_) => match keymap::resolve(FILTER_KEYS, event) {
            Some(action) => {
                act(state, cx, action);
                true
            }
            None => false,
        },
    }
}

fn act(state: &mut State, cx: &mut Cx<'_>, action: Action) {
    let filters = state.filters.len();
    match action {
        Action::Start => state.start(cx),
        Action::Edit => state.focus = FormFocus::Keywords,
        Action::Leave => state.focus = FormFocus::Idle,
        Action::Focus => {
            state.focus = match state.focus {
                FormFocus::Idle | FormFocus::Keywords => FormFocus::Filters(0),
                FormFocus::Filters(_) => FormFocus::Keywords,
            }
        }
        Action::Add => state.open_picker(),
        Action::Remove => {
            if let FormFocus::Filters(index) = state.focus
                && index < filters
            {
                state.filters.remove(index);
                state.focus = FormFocus::Filters(index.min(filters.saturating_sub(2)));
            }
        }
        Action::Prev => {
            if let FormFocus::Filters(index) = state.focus {
                state.focus = FormFocus::Filters(index.saturating_sub(1));
            }
        }
        Action::Next => {
            if let FormFocus::Filters(index) = state.focus {
                state.focus = FormFocus::Filters((index + 1).min(filters.saturating_sub(1)));
            }
        }
        Action::Confirm => {
            if let Adding::Value { key, input } = std::mem::replace(&mut state.adding, Adding::Idle)
            {
                state.add_filter(key, input.value());
                state.focus = FormFocus::Filters(state.filters.len().saturating_sub(1));
            }
        }
        Action::Cancel => state.adding = Adding::Idle,
        Action::Stop => state.stop(cx),
        Action::NewTrace => {
            state.stop(cx);
            state.run = None;
            state.focus = FormFocus::Keywords;
        }
        Action::Up | Action::Down | Action::PageUp | Action::PageDown => {
            if let Some(run) = &mut state.run {
                let len = run.events.len();
                match action {
                    Action::Up => run.timeline.scroll(-1, len),
                    Action::Down => run.timeline.scroll(1, len),
                    Action::PageUp => run.timeline.page(false, len),
                    _ => run.timeline.page(true, len),
                }
            }
        }
        Action::Top => {
            if let Some(run) = &mut state.run {
                run.timeline.home();
            }
        }
        Action::Bottom => {
            if let Some(run) = &mut state.run {
                run.timeline.end();
            }
        }
    }
}

pub fn update(state: &mut State, _cx: &mut Cx<'_>, msg: ScreenMsg<'_, Action>) {
    match msg {
        ScreenMsg::Stream(event) => state.stream_event(event),
        ScreenMsg::Paste(text) => match &mut state.adding {
            Adding::Picking(picker) => {
                picker.paste(text);
            }
            Adding::Value { input, .. } => input.paste(text),
            Adding::Idle if state.run.is_none() && state.focus == FormFocus::Keywords => {
                state.keywords.paste(text)
            }
            Adding::Idle => {}
        },
        ScreenMsg::Key(_)
        | ScreenMsg::Text(_)
        | ScreenMsg::Done(_)
        | ScreenMsg::Tick(_)
        | ScreenMsg::Resumed(_)
        | ScreenMsg::Progress(_) => {}
    }
}

pub fn crumbs(_state: &State, out: &mut Crumbs) {
    out.push("Live tracing");
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/tracing/tests.rs"]
pub(crate) mod tests;
