/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::live::TraceEvent;
use crate::schema::Schema;
use crate::tui::app::{Cx, ScreenMsg, ViewCx};
use crate::tui::exec::Slot;
use crate::tui::job::objects::{GetSpec, ObjectsKind, ObjectsOutput, Reply};
use crate::tui::keymap::{self, Context, code, key, keys, upper};
use crate::tui::msg::JobOutput;
use crate::tui::screens::objects_job;
use crate::tui::session::Connected;
use crate::tui::widgets::chrome::Crumbs;
use crate::tui::widgets::timeline::{Labels, Timeline, write_elapsed};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use serde_json::{Map, Value};

pub const TRACE_OBJECT: &str = "x:Trace";
const EVENTS: &str = "events";
const CARD_HEIGHT: u16 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Reload,
}

keys! { pub KEYS, pub HINTS: Action;
    "↑↓" Up "scroll" true [code(KeyCode::Up), key('k')],
    "↓" Down "scroll down" false [code(KeyCode::Down), key('j')],
    "pgup" PageUp "page up" false [code(KeyCode::PageUp)],
    "pgdn" PageDown "page down" false [code(KeyCode::PageDown)],
    "g" Top "first event" false [key('g'), code(KeyCode::Home)],
    "G" Bottom "last event" false [upper('G'), code(KeyCode::End)],
    "R" Reload "reload" true [upper('R')],
}

pub enum Load {
    Loading,
    Ready,
    Failed(Box<str>),
}

pub struct State {
    object: Box<str>,
    id: Box<str>,
    labels: Labels,
    load: Load,
    events: Vec<TraceEvent>,
    skipped: usize,
    timeline: Timeline,
}

pub fn is_trace(schema: &Schema, view: &str) -> bool {
    schema
        .view(view)
        .is_some_and(|def| def.object.name() == TRACE_OBJECT)
}

impl State {
    pub fn new(conn: &Connected, id: &str) -> Self {
        State {
            object: TRACE_OBJECT.into(),
            id: id.into(),
            labels: Labels::new(conn.schema.clone()),
            load: Load::Loading,
            events: Vec::new(),
            skipped: 0,
            timeline: Timeline::new(false),
        }
    }

    pub fn start(&mut self, cx: &mut Cx<'_>) {
        self.load = Load::Loading;
        let spec = GetSpec {
            object: self.object.clone(),
            id: self.id.clone(),
            properties: None,
        };
        let ticket = cx.slots.start(Slot::Trace);
        cx.fx
            .spawn(ticket, objects_job(cx, ObjectsKind::Get(Box::new(spec))));
    }

    fn loaded(&mut self, result: Reply<Option<Map<String, Value>>>) {
        match result.map(|object| *object) {
            Ok(Some(mut object)) => {
                self.events.clear();
                self.skipped = parse_events(object.remove(EVENTS), &mut self.events);
                self.timeline.reset(false);
                self.load = Load::Ready;
            }
            Ok(None) => self.load = Load::Failed("Trace not found".into()),
            Err(error) => self.load = Load::Failed(format!("Failed to load trace: {error}").into()),
        }
    }
}

pub fn parse_events(value: Option<Value>, out: &mut Vec<TraceEvent>) -> usize {
    let mut skipped = 0;
    let mut push = |value: Value| match serde_json::from_value::<TraceEvent>(value) {
        Ok(event) => out.push(event),
        Err(_) => skipped += 1,
    };
    match value {
        Some(Value::Array(items)) => items.into_iter().for_each(&mut push),
        Some(Value::Object(map)) => {
            let mut items = map
                .into_iter()
                .map(|(key, value)| (key.parse::<u64>().unwrap_or(u64::MAX), value))
                .collect::<Vec<_>>();
            items.sort_unstable_by_key(|(index, _)| *index);
            items
                .into_iter()
                .map(|(_, value)| value)
                .for_each(&mut push);
        }
        _ => {}
    }
    skipped
}

pub fn context(_state: &State) -> Context {
    Context::TraceDetail
}

pub fn handle_key(state: &mut State, cx: &mut Cx<'_>, event: KeyEvent) -> bool {
    let Some(action) = keymap::resolve(KEYS, event) else {
        return false;
    };
    let len = state.events.len();
    match action {
        Action::Up => state.timeline.scroll(-1, len),
        Action::Down => state.timeline.scroll(1, len),
        Action::PageUp => state.timeline.page(false, len),
        Action::PageDown => state.timeline.page(true, len),
        Action::Top => state.timeline.home(),
        Action::Bottom => state.timeline.end(),
        Action::Reload => state.start(cx),
    }
    true
}

pub fn update(state: &mut State, _cx: &mut Cx<'_>, msg: ScreenMsg<'_, Action>) {
    if let ScreenMsg::Done(JobOutput::Objects(ObjectsOutput::Get(result))) = msg {
        state.loaded(result);
    }
}

pub fn crumbs(state: &State, out: &mut Crumbs) {
    out.push("Trace");
    out.push(&state.id);
}

pub fn render(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    let outer = area;
    let block = theme.panel(" Trace ", cx.focused);
    let area = block.inner(outer).inner(Margin::new(1, 0));
    match &state.load {
        Load::Loading => {
            frame.render_widget(block, outer);
            frame.render_widget(Line::styled("loading…", theme.muted()), area);
            return;
        }
        Load::Failed(message) => {
            frame.render_widget(block, outer);
            frame.render_widget(Line::styled(&**message, Style::new().fg(theme.err)), area);
            return;
        }
        Load::Ready => {}
    }
    let [cards, note, body] = Layout::vertical([
        Constraint::Length(CARD_HEIGHT),
        Constraint::Length(u16::from(state.skipped > 0)),
        Constraint::Fill(1),
    ])
    .areas(area);
    let first = state.events.first();
    let last = state.events.last();
    let span = first.map_or("-", |event| state.labels.event_label(&event.event));
    let mut date = String::new();
    match first {
        Some(event) => {
            let _ = event.timestamp.format_iso(&mut date);
        }
        None => date.push('-'),
    }
    let mut duration = String::new();
    match (first, last) {
        (Some(first), Some(last)) if state.events.len() >= 2 => write_elapsed(
            &mut duration,
            last.timestamp
                .millis()
                .saturating_sub(first.timestamp.millis()),
        ),
        _ => duration.push('-'),
    }
    let count = state.events.len().to_string();
    let values = [
        ("Span Type", span),
        ("Date", date.as_str()),
        ("Duration", duration.as_str()),
        ("Events", count.as_str()),
    ];
    let areas = Layout::horizontal([Constraint::Fill(1); 4]).split(cards);
    for ((title, value), area) in values.iter().zip(areas.iter()) {
        frame.render_widget(
            Paragraph::new(Line::styled(*value, theme.strong())).block(theme.panel(*title, false)),
            *area,
        );
    }
    if state.skipped > 0 {
        frame.render_widget(
            Line::styled(
                format!("{} events could not be read", state.skipped),
                Style::new().fg(theme.warn),
            ),
            note,
        );
    }
    let anchor = first.map(|event| event.timestamp);
    state
        .timeline
        .render(frame, body, theme, &state.labels, &state.events, anchor);
    let block = if state.events.is_empty() {
        block
    } else {
        block.title_bottom(
            Line::styled(
                format!(
                    " event {} of {} ",
                    state.timeline.position() + 1,
                    state.events.len()
                ),
                theme.muted(),
            )
            .right_aligned(),
        )
    };
    frame.render_widget(block, outer);
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/screens/trace_detail/tests.rs"]
mod tests;
