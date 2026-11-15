/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::plan::{OnError, Secrets};
use crate::tui::app::{Cx, ScreenMsg, ViewCx};
use crate::tui::exec::{Cancel, Slot};
use crate::tui::form::layer::{Layer, LayerEvent, Layers, Purpose};
use crate::tui::job::objects::{ObjectsKind, ObjectsOutput, ObjectsSlot};
use crate::tui::job::tools::{ApplyMode, ApplySpec, Outcome, SnapshotSpec, ToolReport};
use crate::tui::keymap::{self, Context, code, ctrl, key, keys, upper};
use crate::tui::msg::{JobOutput, JobProgress};
use crate::tui::route::Route;
use crate::tui::screens::objects_job;
use crate::tui::theme::{Level, Theme};
use crate::tui::widgets::chrome::Crumbs;
use crate::tui::widgets::dialog::{CONFIRM_APPLY, CONFIRM_LEAVE, Dialog, Risk};
use crate::tui::widgets::input::Input;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use serde_json::Value;
use std::path::PathBuf;

pub const RUNNING: &str = "A tool is running; c cancels it";
const TITLE: &str = "Tools";
const LABEL_WIDTH: usize = 14;
const ROWS: u16 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Activate,
    LogDown,
    LogUp,
    Cancel,
    Back,
}

keys! { pub KEYS, pub HINTS: Action;
    "j/k" Down "move" true [key('j'), code(KeyCode::Down)],
    "k" Up "up" false [key('k'), code(KeyCode::Up)],
    "⏎" Activate "edit / toggle / run" true [code(KeyCode::Enter), code(KeyCode::Char(' '))],
    "J/K" LogDown "scroll output" true [upper('J'), code(KeyCode::PageDown), ctrl('f')],
    "K" LogUp "scroll output up" false [upper('K'), code(KeyCode::PageUp), ctrl('b')],
    "c" Cancel "cancel run" true [key('c')],
    "esc" Back "back" false [code(KeyCode::Esc)],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    PlanFile,
    Mode,
    OnError,
    Apply,
    Objects,
    Output,
    Secrets,
    Snapshot,
}

const ORDER: [Row; 8] = [
    Row::PlanFile,
    Row::Mode,
    Row::OnError,
    Row::Apply,
    Row::Objects,
    Row::Output,
    Row::Secrets,
    Row::Snapshot,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    DryRun,
    Apply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Idle,
    Running,
    Finished(Outcome),
}

pub struct State {
    cursor: usize,
    editing: bool,
    plan: Input,
    objects: Input,
    output: Input,
    mode: Mode,
    on_error: OnError,
    secrets: Secrets,
    status: Status,
    log: String,
    scroll: u16,
    cancel: Cancel,
    layers: Layers,
}

impl State {
    pub fn new() -> Self {
        State {
            cursor: 0,
            editing: false,
            plan: Input::default(),
            objects: Input::default(),
            output: Input::default(),
            mode: Mode::DryRun,
            on_error: OnError::Stop,
            secrets: Secrets::Strip,
            status: Status::Idle,
            log: String::new(),
            scroll: 0,
            cancel: Cancel::default(),
            layers: Layers::default(),
        }
    }

    fn row(&self) -> Row {
        ORDER.get(self.cursor).copied().unwrap_or(Row::PlanFile)
    }

    fn input(&mut self, row: Row) -> Option<&mut Input> {
        match row {
            Row::PlanFile => Some(&mut self.plan),
            Row::Objects => Some(&mut self.objects),
            Row::Output => Some(&mut self.output),
            Row::Mode | Row::OnError | Row::Apply | Row::Secrets | Row::Snapshot => None,
        }
    }

    fn activate(&mut self, cx: &mut Cx<'_>) {
        match self.row() {
            Row::PlanFile | Row::Objects | Row::Output => self.editing = true,
            Row::Mode => {
                self.mode = match self.mode {
                    Mode::DryRun => Mode::Apply,
                    Mode::Apply => Mode::DryRun,
                }
            }
            Row::OnError => {
                self.on_error = match self.on_error {
                    OnError::Stop => OnError::Continue,
                    OnError::Continue => OnError::Stop,
                }
            }
            Row::Secrets => {
                self.secrets = match self.secrets {
                    Secrets::Strip => Secrets::Include,
                    Secrets::Include => Secrets::Strip,
                }
            }
            Row::Apply => self.confirm_apply(cx),
            Row::Snapshot => self.snapshot(cx),
        }
    }

    fn ready(&self, cx: &mut Cx<'_>) -> bool {
        if self.status == Status::Running {
            cx.flash
                .set(Level::Warn, "A tool is already running", cx.now);
            return false;
        }
        true
    }

    fn confirm_apply(&mut self, cx: &mut Cx<'_>) {
        if !self.ready(cx) {
            return;
        }
        let path = self.plan.value().trim();
        if path.is_empty() {
            cx.flash
                .set(Level::Warn, "Enter the plan file first", cx.now);
            return;
        }
        if self.mode == Mode::DryRun {
            self.run(cx, None);
            return;
        }
        let body = vec![
            Line::raw(format!("Apply the plan in {path} to the server?")),
            Line::raw(""),
            Line::raw("Destroys and reconcile deletions in the plan are executed."),
        ];
        self.layers.push(Layer::Dialog {
            dialog: Dialog::confirm("Apply plan", Risk::High, body, CONFIRM_APPLY),
            purpose: Purpose::Apply(Box::new(Value::Null)),
        });
    }

    fn fresh_cancel(&mut self) -> Cancel {
        self.cancel = Cancel::default();
        self.cancel.clone()
    }

    fn run(&mut self, cx: &mut Cx<'_>, kind: Option<ObjectsKind>) {
        let kind = match kind {
            Some(kind) => kind,
            None => ObjectsKind::Apply(Box::new(ApplySpec {
                path: PathBuf::from(self.plan.value().trim()),
                mode: match self.mode {
                    Mode::DryRun => ApplyMode::DryRun,
                    Mode::Apply => ApplyMode::Run(self.on_error),
                },
                cancel: self.fresh_cancel(),
            })),
        };
        let ticket = cx.slots.start(Slot::Objects(ObjectsSlot::Save));
        cx.fx.spawn(ticket, objects_job(cx, kind));
        self.status = Status::Running;
        self.log.clear();
        self.scroll = 0;
    }

    fn cancel_run(&mut self, cx: &mut Cx<'_>) {
        if self.status == Status::Running {
            self.cancel.cancel();
            cx.flash
                .set(Level::Warn, "Cancelling before the next request", cx.now);
        }
    }

    fn confirm_cancel(&mut self, purpose: Purpose) {
        let body = vec![Line::raw(
            "A tool is running. Leave and cancel it before its next request?",
        )];
        self.layers.push(Layer::Dialog {
            dialog: Dialog::confirm("Tool running", Risk::High, body, CONFIRM_LEAVE),
            purpose,
        });
    }

    fn snapshot(&mut self, cx: &mut Cx<'_>) {
        if !self.ready(cx) {
            return;
        }
        let objects: Vec<String> = self
            .objects
            .value()
            .split([',', ' '])
            .filter(|name| !name.is_empty())
            .map(String::from)
            .collect();
        let path = self.output.value().trim();
        if objects.is_empty() || path.is_empty() {
            cx.flash
                .set(Level::Warn, "Enter the objects and the output file", cx.now);
            return;
        }
        let path = PathBuf::from(path);
        let spec = SnapshotSpec {
            objects,
            path,
            secrets: self.secrets,
            cancel: self.fresh_cancel(),
        };
        self.run(cx, Some(ObjectsKind::Snapshot(Box::new(spec))));
    }

    fn finished(&mut self, cx: &mut Cx<'_>, report: ToolReport) {
        cx.conn.pickers.clear();
        let (level, text) = match report.outcome {
            Outcome::Succeeded => (Level::Ok, "Finished"),
            Outcome::Failed => (Level::Error, "Finished with errors"),
            Outcome::Cancelled => (Level::Warn, "Cancelled"),
        };
        cx.flash.set(level, text, cx.now);
        self.status = Status::Finished(report.outcome);
        self.log = report.log.into_string();
    }
}

impl Default for State {
    fn default() -> Self {
        State::new()
    }
}

pub fn cancel_running(state: &State) {
    if state.status == Status::Running {
        state.cancel.cancel();
    }
}

pub fn is_running(state: &State) -> bool {
    state.status == Status::Running
}

pub fn guard_leave(state: &mut State, route: Route) -> Option<Route> {
    if !is_running(state) {
        return Some(route);
    }
    state.confirm_cancel(Purpose::Leave(Box::new(route)));
    None
}

pub fn guard_reload(state: &mut State) -> bool {
    if !is_running(state) {
        return true;
    }
    state.confirm_cancel(Purpose::Reload);
    false
}

pub fn context(state: &State) -> Context {
    state.layers.context().unwrap_or(Context::Tools)
}

pub fn handle_key(state: &mut State, cx: &mut Cx<'_>, event: KeyEvent) -> bool {
    if !state.layers.is_empty() {
        match state.layers.handle(event) {
            LayerEvent::Confirmed(Purpose::Apply(_)) => state.run(cx, None),
            LayerEvent::Confirmed(Purpose::Leave(route)) => {
                cancel_running(state);
                cx.router.jump(*route);
            }
            LayerEvent::Confirmed(Purpose::Reload) => {
                cancel_running(state);
                cx.router.reload();
            }
            LayerEvent::Pending
            | LayerEvent::Closed(_)
            | LayerEvent::Confirmed(_)
            | LayerEvent::Chosen { .. } => {}
        }
        return true;
    }
    if state.editing {
        if matches!(event.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Tab) {
            state.editing = false;
        } else if let Some(input) = state.input(state.row()) {
            input.handle(event);
        }
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
        ScreenMsg::Paste(text) => {
            let row = state.row();
            if let Some(input) = state.input(row) {
                input.paste(text);
            }
        }
        ScreenMsg::Done(JobOutput::Objects(ObjectsOutput::Tool(report))) => {
            state.finished(cx, *report)
        }
        ScreenMsg::Progress(JobProgress::Log(lines)) => {
            if state.status == Status::Running {
                state.log.push_str(&lines);
            }
        }
        ScreenMsg::Text(_)
        | ScreenMsg::Progress(JobProgress::Bulk(_))
        | ScreenMsg::Done(_)
        | ScreenMsg::Stream(_)
        | ScreenMsg::Tick(_)
        | ScreenMsg::Resumed(_) => {}
    }
}

fn act(state: &mut State, cx: &mut Cx<'_>, action: Action) {
    match action {
        Action::Up => state.cursor = state.cursor.saturating_sub(1),
        Action::Down => state.cursor = (state.cursor + 1).min(ORDER.len() - 1),
        Action::Activate => state.activate(cx),
        Action::LogDown => state.scroll = state.scroll.saturating_add(ROWS),
        Action::LogUp => state.scroll = state.scroll.saturating_sub(ROWS),
        Action::Cancel => state.cancel_run(cx),
        Action::Back if is_running(state) => state.cancel_run(cx),
        Action::Back => cx.back(),
    }
}

pub fn render(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    let block = theme.panel(Line::from(TITLE), cx.focused && state.layers.is_empty());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [form, output] =
        Layout::vertical([Constraint::Length(ROWS), Constraint::Fill(1)]).areas(inner);
    let width = form
        .width
        .saturating_sub(u16::try_from(LABEL_WIDTH + 4).unwrap_or(0));
    let selected = state.row();
    let editing = state.editing;
    let mode = match state.mode {
        Mode::DryRun => "dry run (nothing is changed)",
        Mode::Apply => "apply",
    };
    let on_error = match state.on_error {
        OnError::Stop => "stop at the first failure",
        OnError::Continue => "continue",
    };
    let secrets = match state.secrets {
        Secrets::Strip => "strip",
        Secrets::Include => "include",
    };
    let status = match state.status {
        Status::Idle => "",
        Status::Running => "running…",
        Status::Finished(Outcome::Succeeded) => "finished",
        Status::Finished(Outcome::Failed) => "finished with errors",
        Status::Finished(Outcome::Cancelled) => "cancelled",
    };
    let button = |label: &'static str| Line::styled(label, theme.selected);
    let mut lines = vec![Line::styled("Apply a plan file", theme.title())];
    let plan = state
        .plan
        .line(theme, width, editing && selected == Row::PlanFile);
    lines.push(field(theme, selected == Row::PlanFile, "Plan file", plan));
    lines.push(field(theme, selected == Row::Mode, "Mode", Line::raw(mode)));
    lines.push(field(
        theme,
        selected == Row::OnError,
        "On error",
        Line::raw(on_error),
    ));
    lines.push(field(theme, selected == Row::Apply, "", button("[ Run ]")));
    lines.push(Line::raw(""));
    lines.push(Line::styled("Snapshot to a file", theme.title()));
    let objects = state
        .objects
        .line(theme, width, editing && selected == Row::Objects);
    lines.push(field(theme, selected == Row::Objects, "Objects", objects));
    let out = state
        .output
        .line(theme, width, editing && selected == Row::Output);
    lines.push(field(theme, selected == Row::Output, "Output file", out));
    lines.push(field(
        theme,
        selected == Row::Secrets,
        "Secrets",
        Line::raw(secrets),
    ));
    lines.push(field(
        theme,
        selected == Row::Snapshot,
        "",
        button("[ Write ]"),
    ));
    lines.push(Line::styled(status, theme.muted()));
    frame.render_widget(Paragraph::new(lines), form);
    let style = match state.status {
        Status::Finished(Outcome::Failed) => Style::new().fg(theme.err),
        _ => theme.text(),
    };
    if state.status == Status::Running {
        let lines = u16::try_from(state.log.lines().count()).unwrap_or(u16::MAX);
        state.scroll = lines.saturating_sub(output.height);
    }
    frame.render_widget(
        Paragraph::new(state.log.as_str())
            .style(style)
            .wrap(Wrap { trim: false })
            .scroll((state.scroll, 0)),
        output,
    );
    state.layers.render(frame, area, theme);
}

fn field<'a>(theme: &Theme, selected: bool, label: &'static str, value: Line<'a>) -> Line<'a> {
    let marker = if selected { "▌ " } else { "  " };
    let mut spans = Vec::with_capacity(value.spans.len() + 2);
    spans.push(Span::styled(marker, theme.selected));
    spans.push(Span::styled(
        format!("{label:>LABEL_WIDTH$}  "),
        theme.title(),
    ));
    spans.extend(value.spans);
    Line::from(spans)
}

pub fn crumbs(_state: &State, out: &mut Crumbs) {
    out.push(TITLE);
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/tools/tests.rs"]
mod tests;
