/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::account::Op;
use crate::schema::Schema;
use crate::schema::{Dashboard, DashboardItem};
use crate::tui::app::{Cx, Focus, ScreenMsg};
use crate::tui::exec::{Cancel, Slot};
use crate::tui::form::model::allowed;
use crate::tui::job::metrics::{
    CacheKey, MetricsJob, MetricsOutput, NODE_OBJECT, RangeKey, node_options,
};
use crate::tui::keymap::{self, Binding, Context, code, key, keys, shifted, upper};
use crate::tui::msg::{Effect, Job, JobOutput, StreamEvent, StreamId, StreamSpec};
use crate::tui::prefs::Period;
use crate::tui::session::Connected;
use crate::tui::theme::Level;
use crate::tui::widgets::chrome::Crumbs;
use crate::tui::widgets::input::Input;
use crate::tui::widgets::picker::{self, Picker, PickerItem, Selection};
use crate::view::metrics::plan::{DashboardPlan, LiveAgg};
use crate::view::metrics::{DashboardData, RangePreset, Window};
use crate::view::time::UtcTimestamp;
use grid::Placed;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

mod grid;
mod view;

pub use view::render;

const LIVE_INTERVAL_S: u32 = 30;
const ALL_NODES: &str = "*";
pub const RANGE_FORMAT: &str =
    "Enter the range as two UTC bounds, each YYYY-MM-DD or YYYY-MM-DDTHH:MM.";
const RANGE_TEXT_CAPACITY: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Left,
    Right,
    Up,
    Down,
    Expand,
    Restore,
    Range,
    NextTab,
    PrevTab,
    Freeze,
    Refresh,
    Node,
    CustomRange,
    Confirm,
    Cancel,
}

keys! { pub KEYS, pub HINTS: Action;
    "←↑↓→" Up "move between panels; ↑ from the top row selects dashboards" true [code(KeyCode::Up), key('k')],
    "↓" Down "panel below, or back to the panels" false [code(KeyCode::Down), key('j')],
    "←" Left "panel or dashboard left, then the sidebar" false [code(KeyCode::Left), key('h')],
    "→" Right "panel or dashboard right" false [code(KeyCode::Right), key('l')],
    "e" Expand "expand panel" true [key('e'), code(KeyCode::Enter)],
    "t" Range "range" true [key('t')],
    "]" NextTab "next dashboard" true [key(']'), code(KeyCode::Tab)],
    "[" PrevTab "previous dashboard" false [key('['), shifted(KeyCode::BackTab)],
    "f" Freeze "freeze / resume" true [key('f')],
    "r" Refresh "refresh" false [key('r')],
    "T" CustomRange "custom from/to range" false [upper('T')],
    "n" Node "node" false [key('n')],
}

keys! { pub EXPANDED_KEYS, pub EXPANDED_HINTS: Action;
    "e esc" Restore "restore panel" true [key('e'), code(KeyCode::Enter), code(KeyCode::Esc)],
    "t" Range "range" true [key('t')],
    "f" Freeze "freeze / resume" true [key('f')],
    "r" Refresh "refresh" false [key('r')],
    "T" CustomRange "custom from/to range" false [upper('T')],
    "n" Node "node" false [key('n')],
}

keys! { pub RANGE_KEYS, pub RANGE_HINTS: Action;
    "⏎" Confirm "apply: from and to, space separated, UTC YYYY-MM-DD[THH:MM]" true [code(KeyCode::Enter)],
    "esc" Cancel "cancel" true [code(KeyCode::Esc)],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Tabs,
    Panels,
}

pub enum Popup {
    None,
    Range(Input),
    Nodes(Box<Picker>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LiveStatus {
    #[default]
    Idle,
    Connecting,
    Open,
    Reconnecting,
    Failed(Box<str>),
}

pub enum Load {
    Waiting,
    Ready(DashboardData),
    Failed(Box<str>),
}

impl Drop for State {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub struct State {
    schema: Arc<Schema>,
    pub tab: usize,
    pub range: RangePreset,
    pub window: Window,
    pub focus: usize,
    pub zone: Zone,
    pub expanded: bool,
    pub frozen: bool,
    pub data: Load,
    placed: Vec<Placed>,
    rects: Vec<Rect>,
    scroll: u16,
    buffers: view::Buffers,
    pub node: Option<u64>,
    pub custom: Option<Window>,
    pub popup: Popup,
    live: LiveAgg,
    pub stream: LiveStatus,
    cancel: Cancel,
    key: Option<CacheKey>,
}

impl State {
    pub fn new(conn: &Connected, id: Option<&str>, period: Period, now: UtcTimestamp) -> Self {
        let schema = conn.schema.clone();
        let tab = id
            .and_then(|id| schema.dashboards.iter().position(|d| d.id == id))
            .unwrap_or(0);
        let (range, custom) = match period {
            Period::Preset { preset } => (preset, None),
            Period::Custom { from, to } => (
                RangePreset::default(),
                Window::custom(UtcTimestamp(from), UtcTimestamp(to)),
            ),
        };
        let mut state = State {
            schema,
            tab,
            range,
            window: custom.unwrap_or_else(|| Window::preset(range, now)),
            focus: 0,
            zone: Zone::Panels,
            expanded: false,
            frozen: false,
            data: Load::Waiting,
            placed: Vec::new(),
            rects: Vec::new(),
            scroll: 0,
            buffers: view::Buffers::default(),
            node: None,
            custom,
            popup: Popup::None,
            live: LiveAgg::default(),
            stream: LiveStatus::Idle,
            cancel: Cancel::default(),
            key: None,
        };
        state.select(tab);
        state
    }

    pub fn period(&self) -> Period {
        match self.custom {
            Some(window) => Period::Custom {
                from: window.from.millis(),
                to: window.to.millis(),
            },
            None => Period::Preset { preset: self.range },
        }
    }

    pub fn dashboard(&self) -> Option<&Dashboard> {
        self.schema.dashboards.get(self.tab)
    }

    pub fn items(&self) -> &[DashboardItem] {
        self.dashboard().map_or(&[], Dashboard::items)
    }

    fn select(&mut self, tab: usize) {
        self.tab = tab;
        self.focus = 0;
        self.scroll = 0;
        self.expanded = false;
        self.data = Load::Waiting;
        let Some(dashboard) = self.schema.dashboards.get(tab) else {
            self.placed.clear();
            return;
        };
        grid::place(dashboard.items(), &mut self.placed);
    }

    fn cycle_tab(&mut self, forward: bool) {
        let count = self.schema.dashboards.len();
        if count > 1 {
            let tab = if forward {
                (self.tab + 1) % count
            } else {
                (self.tab + count - 1) % count
            };
            self.select(tab);
        }
    }

    fn step_tab(&mut self, forward: bool) -> bool {
        let tab = if forward {
            self.tab + 1
        } else {
            match self.tab.checked_sub(1) {
                Some(tab) => tab,
                None => return false,
            }
        };
        if tab >= self.schema.dashboards.len() {
            return false;
        }
        self.select(tab);
        true
    }

    fn move_focus(&mut self, dx: i32, dy: i32) -> bool {
        match grid::neighbour(&self.rects, self.focus, dx, dy) {
            Some(next) => {
                self.focus = next;
                true
            }
            None => false,
        }
    }

    pub fn start(&mut self, cx: &mut Cx<'_>) {
        self.cancel.cancel();
        self.cancel = Cancel::default();
        cx.slots.bump(&Slot::Dashboard);
        cx.slots.bump(&Slot::Stream);
        let schema = self.schema.clone();
        let Some(dashboard) = schema.dashboards.get(self.tab) else {
            self.data = Load::Ready(DashboardData::default());
            return;
        };
        let key = CacheKey {
            dashboard: dashboard.id.as_str().into(),
            range: match self.custom {
                Some(window) => RangeKey::Custom {
                    from: window.from.millis(),
                    to: window.to.millis(),
                },
                None => RangeKey::Preset(self.range),
            },
            node: self.node,
        };
        if cx.conn.metrics.nodes_stale(cx.now)
            && allowed(&schema, &cx.conn.info.permissions, NODE_OBJECT, Op::Get)
        {
            let ticket = cx.slots.start(Slot::Metrics);
            cx.fx.spawn(
                ticket,
                Job::Metrics(MetricsJob::Nodes {
                    client: cx.conn.client.clone(),
                }),
            );
        }
        let plan = DashboardPlan::new(dashboard, self.window, self.node);
        self.live = plan.live();
        let live = plan
            .live_metrics()
            .map(Box::from)
            .collect::<Box<[Box<str>]>>();
        self.stream = LiveStatus::Idle;
        if !live.is_empty() {
            self.stream = LiveStatus::Connecting;
            let stream = StreamId::start(cx.slots);
            cx.fx.push(Effect::OpenStream(
                stream,
                StreamSpec::Metrics {
                    ids: live,
                    interval_s: LIVE_INTERVAL_S,
                },
            ));
        }
        if let Some((window, data)) = cx.conn.metrics.get(&key, cx.now) {
            self.window = window;
            let mut data = data.clone();
            self.live.fill(&mut data);
            self.data = Load::Ready(data);
            self.key = None;
        } else if plan.has_history() {
            self.data = Load::Waiting;
            self.key = Some(key);
            let ticket = cx.slots.start(Slot::Dashboard);
            cx.fx.spawn(
                ticket,
                Job::Metrics(MetricsJob::History {
                    client: cx.conn.client.clone(),
                    plan: Arc::new(plan),
                    cancel: self.cancel.clone(),
                }),
            );
        } else {
            let mut data = plan.history().finish();
            self.live.fill(&mut data);
            self.data = Load::Ready(data);
            self.key = None;
        }
    }

    fn history(&mut self, cx: &mut Cx<'_>, output: MetricsOutput) {
        match output {
            MetricsOutput::History(Ok(data)) => {
                let mut data = *data;
                cx.conn.metrics.see_nodes(&data.nodes);
                if let Some(key) = self.key.take() {
                    cx.conn.metrics.put(key, self.window, data.clone(), cx.now);
                }
                self.live.fill(&mut data);
                self.data = Load::Ready(data);
            }
            MetricsOutput::History(Err(error)) => {
                self.data = Load::Failed(error.to_string().into());
            }
            MetricsOutput::Nodes(names) => cx.conn.metrics.set_nodes(names, cx.now),
        }
    }

    fn live_frame(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::Open => self.stream = LiveStatus::Open,
            StreamEvent::Reconnecting { .. } => self.stream = LiveStatus::Reconnecting,
            StreamEvent::Closed(Some(error)) => {
                self.stream = LiveStatus::Failed(error.to_string().into());
            }
            StreamEvent::Closed(None) => self.stream = LiveStatus::Idle,
            StreamEvent::Metrics(samples) => {
                self.stream = LiveStatus::Open;
                if !self.frozen {
                    self.live.apply(&samples);
                    if let Load::Ready(data) = &mut self.data {
                        self.live.fill(data);
                    }
                }
            }
            StreamEvent::Trace(_) | StreamEvent::Stage(_) => {}
        }
    }

    fn open_nodes(&mut self, conn: &Connected) {
        let options = node_options(
            conn.metrics.node_names(),
            conn.metrics.seen_nodes(),
            self.node,
        );
        if options.len() < 2 && self.node.is_none() {
            return;
        }
        let items = std::iter::once(PickerItem::new(ALL_NODES, "All nodes", ""))
            .chain(
                options
                    .into_iter()
                    .map(|(id, label)| PickerItem::new(id.to_string(), label, "")),
            )
            .collect();
        let current = self.node.map(|node| node.to_string());
        self.popup = Popup::Nodes(Box::new(
            Picker::new("Node", items, Selection::Single)
                .with_current(Some(current.as_deref().unwrap_or(ALL_NODES))),
        ));
    }

    fn apply_range(&mut self, cx: &mut Cx<'_>, text: &str) -> bool {
        let mut bounds = text.split_whitespace().filter(|part| *part != "to");
        let window = match (bounds.next(), bounds.next(), bounds.next()) {
            (Some(from), Some(to), None) => {
                let from =
                    UtcTimestamp::parse_date(from).or_else(|| UtcTimestamp::parse_minute(from));
                let to = UtcTimestamp::parse_date(to)
                    .map(UtcTimestamp::next_day_start)
                    .or_else(|| UtcTimestamp::parse_minute(to));
                from.zip(to).and_then(|(from, to)| Window::custom(from, to))
            }
            _ => None,
        };
        match window {
            Some(window) => {
                self.custom = Some(window);
                self.reload();
                self.start(cx);
                true
            }
            None => {
                cx.flash.set(Level::Error, RANGE_FORMAT, cx.now);
                false
            }
        }
    }

    fn reload(&mut self) {
        self.window = match self.custom {
            Some(window) => window,
            None => Window::preset(self.range, UtcTimestamp::from_system(SystemTime::now())),
        };
        self.data = Load::Waiting;
    }
}

pub fn context(state: &State) -> Context {
    match &state.popup {
        Popup::Range(_) => Context::DashboardRange,
        Popup::Nodes(_) => Context::Picker,
        Popup::None if state.expanded => Context::DashboardExpanded,
        Popup::None => Context::Dashboard,
    }
}

pub fn handle_key(state: &mut State, cx: &mut Cx<'_>, event: KeyEvent) -> bool {
    match &mut state.popup {
        Popup::Nodes(picker) => {
            match picker.handle(event) {
                picker::Outcome::Chosen(index) => {
                    let node = picker.item(index).map(|item| item.key.parse::<u64>().ok());
                    state.popup = Popup::None;
                    if let Some(node) = node {
                        state.node = node;
                        state.reload();
                        state.start(cx);
                    }
                }
                picker::Outcome::Cancel => state.popup = Popup::None,
                picker::Outcome::Pending
                | picker::Outcome::QueryChanged
                | picker::Outcome::ChosenMany(_) => {}
            }
            true
        }
        Popup::Range(input) => {
            match keymap::resolve(RANGE_KEYS, event) {
                Some(Action::Confirm) => {
                    let text = input.value().to_owned();
                    if state.apply_range(cx, &text) {
                        state.popup = Popup::None;
                    }
                }
                Some(_) => state.popup = Popup::None,
                None => {
                    input.handle(event);
                }
            }
            true
        }
        Popup::None => match keymap::resolve(keys(state), event) {
            Some(action) => {
                update(state, cx, ScreenMsg::Key(action));
                true
            }
            None => false,
        },
    }
}

pub fn format_window(window: &Window, separator: &str, out: &mut String) -> fmt::Result {
    let Window { from, to, .. } = *window;
    if from.day_start() == from {
        from.format_date(out)?;
    } else {
        from.format_minute(out)?;
    }
    out.push_str(separator);
    if to.day_start() == to {
        to.add_millis(-1).format_date(out)
    } else {
        to.format_minute(out)
    }
}

pub fn keys(state: &State) -> &'static [Binding<Action>] {
    if state.expanded { EXPANDED_KEYS } else { KEYS }
}

pub fn update(state: &mut State, cx: &mut Cx<'_>, msg: ScreenMsg<'_, Action>) {
    let action = match msg {
        ScreenMsg::Key(action) => action,
        ScreenMsg::Done(JobOutput::Metrics(output)) => return state.history(cx, output),
        ScreenMsg::Stream(event) => return state.live_frame(event),
        _ => return,
    };
    match (action, state.zone) {
        (Action::Left, Zone::Tabs) => {
            if state.step_tab(false) {
                state.start(cx);
            } else {
                *cx.focus = Focus::Sidebar;
            }
        }
        (Action::Right, Zone::Tabs) => {
            if state.step_tab(true) {
                state.start(cx);
            }
        }
        (Action::Down | Action::Expand, Zone::Tabs) => state.zone = Zone::Panels,
        (Action::Up, Zone::Tabs) => {}
        (Action::Left, Zone::Panels) => {
            if !state.move_focus(-1, 0) {
                *cx.focus = Focus::Sidebar;
            }
        }
        (Action::Right, Zone::Panels) => {
            state.move_focus(1, 0);
        }
        (Action::Up, Zone::Panels) => {
            if !state.move_focus(0, -1) {
                state.zone = Zone::Tabs;
            }
        }
        (Action::Down, Zone::Panels) => {
            state.move_focus(0, 1);
        }
        (Action::Expand, _) => state.expanded = !state.items().is_empty(),
        (Action::Restore, _) => state.expanded = false,
        (Action::NextTab, _) => {
            state.cycle_tab(true);
            state.start(cx);
        }
        (Action::PrevTab, _) => {
            state.cycle_tab(false);
            state.start(cx);
        }
        (Action::Node, _) => state.open_nodes(cx.conn),
        (Action::CustomRange, _) => {
            let mut text = String::with_capacity(RANGE_TEXT_CAPACITY);
            let _ = format_window(&state.window, " ", &mut text);
            state.popup = Popup::Range(Input::new(&text));
        }
        (Action::Confirm | Action::Cancel, _) => {}
        (Action::Range, _) => {
            state.custom = None;
            state.range = RangePreset::ALL
                .iter()
                .cycle()
                .skip_while(|r| **r != state.range)
                .nth(1)
                .copied()
                .unwrap_or_default();
            state.reload();
            state.start(cx);
        }
        (Action::Refresh, _) => {
            state.reload();
            cx.conn.metrics = Default::default();
            state.start(cx);
        }
        (Action::Freeze, _) => {
            state.frozen = !state.frozen;
            let text = if state.frozen {
                "live updates frozen"
            } else {
                "live updates resumed"
            };
            cx.flash.set(Level::Info, text, cx.now);
        }
    }
}

pub fn crumbs(state: &State, out: &mut Crumbs) {
    out.push("Dashboard");
    if let Some(dashboard) = state.dashboard() {
        out.push(&dashboard.label);
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/dashboard/tests.rs"]
mod tests;
