/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Error;
use crate::jmap::live::{DeliveryStage, Severity, StageKind};
use crate::tui::app::{Cx, ScreenMsg};
use crate::tui::exec::Slot;
use crate::tui::keymap::{self, Context, code, key, keys};
use crate::tui::msg::{Effect, StreamEvent, StreamId, StreamSpec};
use crate::tui::widgets::chrome::Crumbs;
use crate::tui::widgets::input::Input;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::time::{Duration, Instant};

mod stage;
mod view;

pub use stage::{Phase, valid_target};
pub use view::render;

pub const CUTOFF: Duration = Duration::from_secs(120);
pub const TIMED_OUT: &str = "Trace timed out after 120 seconds.";
pub const INVALID_TARGET: &str = "Enter a valid email address or domain.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Edit,
    Leave,
    Cancel,
    NewTrace,
    Details,
    Up,
    Down,
}

keys! { pub IDLE_KEYS, pub IDLE_HINTS: Action;
    "⏎" Edit "enter a target" true [code(KeyCode::Enter), key('e'), key('i')],
}

keys! { pub INPUT_KEYS, pub INPUT_HINTS: Action;
    "⏎" Start "start trace" true [code(KeyCode::Enter)],
    "esc" Leave "leave the field" true [code(KeyCode::Esc)],
}

keys! { pub KEYS, pub HINTS: Action;
    "c" Cancel "cancel trace" true [key('c')],
    "n" NewTrace "run another trace" true [key('n')],
    "d" Details "toggle details" true [key('d')],
    "↑↓" Up "scroll" true [code(KeyCode::Up), key('k')],
    "↓" Down "scroll down" false [code(KeyCode::Down), key('j')],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Completed,
    Failed(Box<str>),
}

pub struct Row {
    pub stage: DeliveryStage,
    pub phase: Phase,
}

pub struct Run {
    stream: StreamId,
    pub target: Box<str>,
    pub started: Instant,
    pub finished: Option<Instant>,
    pub status: Status,
    opened: bool,
    pub rows: Vec<Row>,
    pending: [Option<usize>; Phase::COUNT],
    pub attempts: u32,
    pub delivered: bool,
    pub scroll: u16,
}

impl Run {
    fn push(&mut self, stage: DeliveryStage) {
        let phase = Phase::of(stage.kind);
        match stage.kind {
            StageKind::DeliveryAttemptStart => {
                self.attempts += 1;
                self.pending = [None; Phase::COUNT];
            }
            StageKind::RcptToSuccess | StageKind::QuitCompleted => self.delivered = true,
            _ => {}
        }
        let slot = self.pending.get_mut(phase.index());
        let row = Row { stage, phase };
        match (row.stage.kind.severity(), slot) {
            (Severity::Pending, Some(slot)) if phase != Phase::Attempt => {
                *slot = Some(self.rows.len());
                self.rows.push(row);
            }
            (_, Some(slot)) if phase != Phase::Attempt => {
                match slot.take().and_then(|index| self.rows.get_mut(index)) {
                    Some(pending) => *pending = row,
                    None => self.rows.push(row),
                }
            }
            _ => self.rows.push(row),
        }
    }
}

pub struct State {
    pub target: Input,
    pub editing: bool,
    pub invalid: bool,
    pub details: bool,
    pub run: Option<Run>,
}

impl State {
    pub fn new() -> Self {
        State {
            target: Input::default(),
            editing: false,
            invalid: false,
            details: false,
            run: None,
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        let run = self.run.as_ref().filter(|r| r.status == Status::Running)?;
        let elapsed = run.started.elapsed();
        let next = Duration::from_secs(elapsed.as_secs() + 1);
        Some(run.started + next.min(CUTOFF))
    }

    fn start(&mut self, cx: &mut Cx<'_>) {
        let target = self.target.value().trim();
        if !valid_target(target) {
            self.invalid = true;
            return;
        }
        let stream = StreamId::start(cx.slots);
        let target: Box<str> = target.into();
        cx.fx.push(Effect::OpenStream(
            stream,
            StreamSpec::Delivery {
                target: target.clone(),
            },
        ));
        self.run = Some(Run {
            stream,
            target,
            started: cx.now,
            finished: None,
            status: Status::Running,
            opened: false,
            rows: Vec::new(),
            pending: [None; Phase::COUNT],
            attempts: 0,
            delivered: false,
            scroll: 0,
        });
    }

    fn finish(&mut self, cx: &mut Cx<'_>, status: Status) {
        if let Some(run) = &mut self.run
            && run.status == Status::Running
        {
            cx.fx.push(Effect::CloseStream(run.stream));
            cx.slots.bump(&Slot::Stream);
            run.finished = Some(cx.now);
            run.status = status;
        }
    }

    fn stream_event(&mut self, cx: &mut Cx<'_>, event: StreamEvent) {
        let Some(run) = &mut self.run else {
            return;
        };
        if run.status != Status::Running {
            return;
        }
        match event {
            StreamEvent::Open => run.opened = true,
            StreamEvent::Stage(stage) => run.push(*stage),
            StreamEvent::Closed(None) => {
                run.finished = Some(cx.now);
                run.status = Status::Completed;
            }
            StreamEvent::Closed(Some(error)) => {
                run.finished = Some(cx.now);
                run.status = Status::Failed(failure(&error, run.opened));
            }
            StreamEvent::Reconnecting { .. } | StreamEvent::Metrics(_) | StreamEvent::Trace(_) => {}
        }
    }
}

impl Default for State {
    fn default() -> Self {
        State::new()
    }
}

fn failure(error: &Error, opened: bool) -> Box<str> {
    match error {
        Error::PermissionDenied => "You do not have permission to perform this action.".into(),
        Error::NotFound => "The requested resource was not found.".into(),
        _ if opened => format!("Connection lost before trace completed: {error}").into(),
        _ => format!("The server refused the trace stream: {error}").into(),
    }
}

pub fn context(state: &State) -> Context {
    match (&state.run, state.editing) {
        (Some(_), _) => Context::Delivery,
        (None, true) => Context::DeliveryInput,
        (None, false) => Context::DeliveryForm,
    }
}

pub fn available(state: &State, action: Action) -> bool {
    let running = state
        .run
        .as_ref()
        .is_some_and(|run| run.status == Status::Running);
    match action {
        Action::Cancel => running,
        Action::NewTrace => !running,
        _ => true,
    }
}

pub fn handle_key(state: &mut State, cx: &mut Cx<'_>, event: KeyEvent) -> bool {
    let table = match (&state.run, state.editing) {
        (Some(_), _) => KEYS,
        (None, true) => INPUT_KEYS,
        (None, false) => IDLE_KEYS,
    };
    match keymap::resolve(table, event) {
        Some(action) => {
            act(state, cx, action);
            true
        }
        None if state.run.is_none() && state.editing && keymap::is_text(event) => {
            state.invalid = false;
            state.target.handle(event)
        }
        None => false,
    }
}

fn act(state: &mut State, cx: &mut Cx<'_>, action: Action) {
    match action {
        Action::Start => state.start(cx),
        Action::Edit => state.editing = true,
        Action::Leave => state.editing = false,
        Action::Cancel => {
            state.finish(cx, Status::Failed("Trace cancelled.".into()));
            state.run = None;
        }
        Action::NewTrace => {
            state.finish(cx, Status::Failed("Trace cancelled.".into()));
            state.run = None;
            state.target = Input::default();
            state.editing = true;
        }
        Action::Details => state.details = !state.details,
        Action::Up => {
            if let Some(run) = &mut state.run {
                run.scroll = run.scroll.saturating_sub(1);
            }
        }
        Action::Down => {
            if let Some(run) = &mut state.run {
                run.scroll = run.scroll.saturating_add(1);
            }
        }
    }
}

pub fn update(state: &mut State, cx: &mut Cx<'_>, msg: ScreenMsg<'_, Action>) {
    match msg {
        ScreenMsg::Stream(event) => state.stream_event(cx, event),
        ScreenMsg::Tick(now) => {
            if state
                .run
                .as_ref()
                .is_some_and(|run| now.saturating_duration_since(run.started) >= CUTOFF)
            {
                state.finish(cx, Status::Failed(TIMED_OUT.into()));
            }
        }
        ScreenMsg::Paste(text) if state.run.is_none() && state.editing => {
            state.invalid = false;
            state.target.paste(text);
        }
        ScreenMsg::Key(_)
        | ScreenMsg::Text(_)
        | ScreenMsg::Paste(_)
        | ScreenMsg::Done(_)
        | ScreenMsg::Progress(_)
        | ScreenMsg::Resumed(_) => {}
    }
}

pub fn crumbs(_state: &State, out: &mut Crumbs) {
    out.push("Delivery test");
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/delivery/tests.rs"]
mod tests;
