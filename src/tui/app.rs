/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::account::Op;
use crate::jmap::credential::Credential;
use crate::tui::Launch;
use crate::tui::exec::{Slot, SlotTable};
use crate::tui::form::model::allowed;
use crate::tui::job::auth::{AuthJob, AuthTarget};
use crate::tui::keymap::{self, Binding, Context, GlobalAction, global};
use crate::tui::log::HttpLog;
use crate::tui::msg::{Effect, Effects, HttpUrl, Job, JobOutput, JobProgress, Msg, StreamEvent};
use crate::tui::prefs::Prefs;
use crate::tui::route::{Arrival, ObjectRef, Route, RouteTarget, Router};
use crate::tui::screens::bootstrap::Links;
use crate::tui::screens::{
    action, bootstrap, browser, dashboard, delivery, detail, form, log_view, login, message, tools,
    trace_detail, tracing,
};
use crate::tui::session::{Access, Connected};
use crate::tui::theme::{Level, Theme};
use crate::tui::widgets::chrome::{
    Crumbs, FlashSlot, FooterInfo, HeaderInfo, Health, footer, header, help_overlay,
};
use crate::tui::widgets::dialog::{self, Dialog, DialogKey, Risk};
use crate::tui::widgets::palette::{self, Command, PaletteIndex, PaletteState, PaletteTarget};
use crate::tui::widgets::picker::{self, Picker, PickerItem, Selection};
use crate::tui::widgets::sidebar::{self, LayoutTree, SIDEBAR_WIDTH, Sidebar, SidebarMode};
use crate::view::LabelCache;
use crate::view::time::UtcTimestamp;
use ratatui::Frame;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Block;
use std::io;
use std::time::{Instant, SystemTime};

pub enum ScreenMsg<'a, A> {
    Key(A),
    Text(KeyEvent),
    Paste(&'a str),
    Done(JobOutput),
    Stream(StreamEvent),
    Tick(Instant),
    Resumed(Option<&'a io::Error>),
    Progress(JobProgress),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Content,
    Sidebar,
}

pub struct Cx<'a> {
    pub fx: &'a mut Effects,
    pub slots: &'a mut SlotTable,
    pub flash: &'a mut FlashSlot,
    pub overlays: &'a mut Vec<Overlay>,
    pub router: &'a mut Router,
    pub conn: &'a mut Connected,
    pub focus: &'a mut Focus,
    pub now: Instant,
}

impl Cx<'_> {
    pub fn back(&mut self) {
        if !self.router.back() {
            *self.focus = Focus::Sidebar;
        }
    }
}

pub struct LoginCx<'a> {
    pub fx: &'a mut Effects,
    pub slots: &'a mut SlotTable,
    pub flash: &'a mut FlashSlot,
    pub launch: &'a Launch,
    pub connected: &'a mut Option<Connected>,
    pub now: Instant,
}

pub struct ViewCx<'a> {
    pub theme: &'a Theme,
    pub conn: &'a Connected,
    pub tick: u64,
    pub now: Instant,
    pub focused: bool,
}

pub enum Overlay {
    Palette(PaletteState),
    Help(Context),
    Confirm(Box<Dialog>, Confirmable),
    Accounts(Box<Picker>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmable {
    Logout,
}

const MAX_PARKED: usize = 8;
const SWITCH_ACCOUNT: &str = "Switch account";
const NO_ACCOUNTS: &str = "The server lists no accounts for this session";
const JOB_PANICKED: &str = "A background task failed unexpectedly; try again";
const SWITCH_DIRTY: &str = "Save or discard the form changes before switching accounts";
const SESSION_EXPIRED: &str = "Your session has expired; sign in again.";
const CREDENTIALS_REJECTED: &str = "The server rejected the credentials; sign in again.";

pub const CONFIRM_LOGOUT: &[DialogKey] = &[DialogKey {
    key: 'Y',
    label: "sign out (capital Y)",
    danger: false,
}];

pub enum Screen {
    Login(Box<login::State>),
    Browser(Box<browser::State>),
    Dashboard(Box<dashboard::State>),
    Form(Box<form::State>),
    Action(Box<action::State>),
    Tools(Box<tools::State>),
    Bootstrap(Box<bootstrap::State>),
    Detail(Box<detail::State>),
    Tracing(Box<tracing::State>),
    Delivery(Box<delivery::State>),
    TraceDetail(Box<trace_detail::State>),
    Message(message::State),
    Log(log_view::State),
}

pub struct App {
    pub theme: Theme,
    pub launch: Launch,
    pub screen: Screen,
    pub overlays: Vec<Overlay>,
    pub router: Router,
    pub slots: SlotTable,
    pub flash: FlashSlot,
    pub conn: Option<Connected>,
    pub trees: Vec<LayoutTree>,
    pub index: PaletteIndex,
    pub sidebar: Sidebar,
    pub focus: Focus,
    pub health: Health,
    pub log: Option<HttpLog>,
    pub prefs: Prefs,
    pub prefs_dirty: bool,
    pub quit: bool,
    pub dirty: bool,
    started: Instant,
    crumbs: Crumbs,
    resume: Option<Route>,
    parked: Vec<(Route, Box<browser::State>)>,
}

macro_rules! screen_key {
    ($module:ident, $state:expr, $table:expr, $cx:expr, $event:expr, $ctx:expr) => {
        match keymap::resolve($table, $event) {
            Some(action) => {
                $module::update($state, $cx, ScreenMsg::Key(action));
                true
            }
            None if $ctx.captures_text() && keymap::is_text($event) => {
                $module::update($state, $cx, ScreenMsg::Text($event));
                true
            }
            None => false,
        }
    };
}

impl App {
    pub fn new(launch: Launch, prefs: Prefs, now: Instant) -> Self {
        let screen = Screen::Login(Box::new(login::State::new(&launch)));
        let palette = launch.palette.resolve(prefs.theme);
        let sidebar = launch
            .sidebar
            .or(prefs.sidebar)
            .unwrap_or(SidebarMode::Always);
        App {
            theme: Theme::new(palette),
            sidebar: Sidebar::new(sidebar),
            prefs,
            prefs_dirty: false,
            launch,
            screen,
            overlays: Vec::new(),
            router: Router::default(),
            slots: SlotTable::default(),
            flash: FlashSlot::default(),
            conn: None,
            trees: Vec::new(),
            index: PaletteIndex::default(),
            focus: Focus::Content,
            health: Health::Ok,
            log: None,
            quit: false,
            dirty: true,
            started: now,
            crumbs: Crumbs::default(),
            resume: None,
            parked: Vec::new(),
        }
    }

    pub fn start(&mut self, fx: &mut Effects, now: Instant) {
        if let Screen::Login(state) = &mut self.screen {
            let mut cx = LoginCx {
                fx,
                slots: &mut self.slots,
                flash: &mut self.flash,
                launch: &self.launch,
                connected: &mut self.conn,
                now,
            };
            login::start(state, &mut cx);
        }
    }

    pub fn context(&self) -> Context {
        match self.overlays.last() {
            Some(Overlay::Palette(_)) => return Context::Palette,
            Some(Overlay::Help(ctx)) => return *ctx,
            Some(Overlay::Confirm(..)) => return Context::Dialog,
            Some(Overlay::Accounts(_)) => return Context::Picker,
            None => {}
        }
        if self.focus == Focus::Sidebar && self.conn.is_some() {
            return Context::Sidebar;
        }
        self.screen_context()
    }

    fn screen_context(&self) -> Context {
        match &self.screen {
            Screen::Login(state) => login::context(state),
            Screen::Message(state) => message::context(state),
            Screen::Log(state) => log_view::context(state),
            Screen::Browser(state) => browser::context(state),
            Screen::Form(state) => form::context(state),
            Screen::Action(state) => action::context(state),
            Screen::Tools(state) => tools::context(state),
            Screen::Bootstrap(state) => bootstrap::context(state),
            Screen::Detail(state) => detail::context(state),
            Screen::Tracing(state) => tracing::context(state),
            Screen::Delivery(state) => delivery::context(state),
            Screen::TraceDetail(state) => trace_detail::context(state),
            Screen::Dashboard(state) => dashboard::context(state),
        }
    }

    pub fn tick(&self, now: Instant) -> u64 {
        let elapsed = now.saturating_duration_since(self.started);
        u64::try_from(elapsed.as_millis() / crate::tui::widgets::spinner::FRAME.as_millis())
            .unwrap_or(0)
    }

    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let screen = match &self.screen {
            Screen::Login(state) => state.deadline(now),
            Screen::Log(state) => state.deadline(),
            Screen::Delivery(state) => state.deadline(),
            Screen::Browser(state) => browser::deadline(state),
            Screen::Form(state) => form::deadline(state),
            Screen::Action(state) => action::deadline(state),
            Screen::Message(_)
            | Screen::Tools(_)
            | Screen::Bootstrap(_)
            | Screen::Detail(_)
            | Screen::Tracing(_)
            | Screen::TraceDetail(_)
            | Screen::Dashboard(_) => None,
        };
        match (screen, self.flash.deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    pub fn update(&mut self, msg: Msg, fx: &mut Effects, now: Instant) {
        match msg {
            Msg::Input(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                self.dirty = true;
                self.key(key, fx, now);
            }
            Msg::Input(Event::Paste(text)) => {
                self.dirty = true;
                self.paste(&text, fx, now);
            }
            Msg::Input(Event::Resize(..)) => self.dirty = true,
            Msg::Input(_) => {}
            Msg::Done(ticket, output) => {
                if self.slots.is_current(&ticket) {
                    self.dirty = true;
                    self.health = Health::after(output.error());
                    if self.conn.is_some() && output.session_expired() {
                        self.expire();
                    } else {
                        self.screen_msg(Passive::Done(output), fx, now);
                        self.store_warnings(now);
                    }
                }
            }
            Msg::Stream(id, event) => {
                if self.slots.is_current(&id.ticket()) {
                    self.dirty = true;
                    match &event {
                        StreamEvent::Reconnecting { .. } => self.health = Health::Degraded,
                        StreamEvent::Closed(Some(error)) => {
                            if Health::after(Some(error)) == Health::Down {
                                self.health = Health::Down;
                            }
                        }
                        _ => self.health = Health::Ok,
                    }
                    if self.conn.is_some() && event.session_expired() {
                        self.expire();
                    } else {
                        self.screen_msg(Passive::Stream(event), fx, now);
                    }
                }
            }
            Msg::Tick(now) => {
                if self.flash.expire(now) {
                    self.dirty = true;
                }
                let redraw = match &mut self.screen {
                    Screen::Log(state) => state.refresh(now),
                    _ => self.next_deadline(now).is_some(),
                };
                if redraw {
                    self.dirty = true;
                }
                self.screen_msg(Passive::Tick(now), fx, now);
            }
            Msg::Resumed(ticket, error) => {
                self.dirty = true;
                if self.slots.is_current(&ticket) {
                    self.screen_msg(Passive::Resumed(error.as_deref()), fx, now);
                } else if let Some(error) = error {
                    self.flash.set(Level::Error, error.to_string(), now);
                }
            }
            Msg::JobPanicked(ticket) => {
                if self.slots.is_current(&ticket) {
                    self.dirty = true;
                    self.flash.set(Level::Error, JOB_PANICKED, now);
                }
            }
            Msg::InputFailed(_) => self.quit = true,
            Msg::Progress(ticket, progress) => {
                if self.slots.is_current(&ticket) {
                    self.dirty = true;
                    self.screen_msg(Passive::Progress(progress), fx, now);
                }
            }
        }
        self.after_update(fx, now);
    }

    pub fn open_failed(&mut self, url: &HttpUrl, error: &io::Error, now: Instant) {
        self.dirty = true;
        self.flash.set(
            Level::Error,
            format!("Could not open {}: {error}", url.as_str()),
            now,
        );
    }

    fn after_update(&mut self, fx: &mut Effects, now: Instant) {
        if let Screen::Login(_) = self.screen
            && self.conn.is_some()
        {
            self.on_connected();
        }
        if let Some((route, arrival)) = self.router.take_arrival() {
            let restored = match arrival {
                Arrival::Reload => {
                    if let Some(conn) = &mut self.conn {
                        conn.pickers.clear();
                    }
                    None
                }
                Arrival::Back => self.unpark(&route),
                Arrival::Open => {
                    self.park();
                    None
                }
            };
            self.enter(route, restored, fx, now);
        }
        if self.focus == Focus::Sidebar && self.trees.is_empty() {
            self.focus = Focus::Content;
        }
        if let Screen::Dashboard(state) = &self.screen
            && self.prefs.set_period(state.period())
        {
            self.prefs_dirty = true;
        }
        if std::mem::take(&mut self.prefs_dirty) {
            fx.push(Effect::SavePrefs);
        }
        if self.quit {
            fx.push(Effect::Quit);
        }
    }

    fn park(&mut self) {
        let Some(previous) = self.router.previous().filter(|_| self.router.depth() > 0) else {
            self.parked.clear();
            return;
        };
        if !matches!(self.screen, Screen::Browser(_)) {
            return;
        }
        let previous = previous.clone();
        let placeholder = Screen::Message(message::State::unknown(""));
        if let Screen::Browser(state) = std::mem::replace(&mut self.screen, placeholder) {
            if self.parked.len() == MAX_PARKED {
                self.parked.remove(0);
            }
            self.parked.push((previous, state));
        }
    }

    fn unpark(&mut self, route: &Route) -> Option<Box<browser::State>> {
        let index = self
            .parked
            .iter()
            .rposition(|(parked, _)| parked == route)?;
        self.parked.drain(index..).next().map(|(_, state)| state)
    }

    fn on_connected(&mut self) {
        if self.conn.is_none() {
            return;
        }
        self.rebuild_navigation();
        self.router.reset();
        let resumed = self.resume.take().filter(|route| self.reachable(route));
        let route = resumed.or_else(|| self.start_route());
        self.router
            .replace(route.unwrap_or(Route::new(None, RouteTarget::Unknown("".into()))));
    }

    fn rebuild_navigation(&mut self) {
        let Some(conn) = &self.conn else {
            return;
        };
        self.trees = conn
            .schema
            .layouts
            .iter()
            .map(|layout| LayoutTree::build(layout, &mut |view| conn.access(view)))
            .filter(|tree| !tree.nodes.is_empty())
            .collect();
        self.sidebar.reset(&self.trees);
        self.index = PaletteIndex::build(conn, &self.trees, self.log.is_some());
    }

    fn reachable(&self, route: &Route) -> bool {
        match (route.layout, route.link()) {
            (Some(layout), Some(link)) => self
                .trees
                .get(usize::from(layout))
                .and_then(|tree| tree.find(&link))
                .is_some_and(|node| !node.locked),
            _ => true,
        }
    }

    fn start_route(&self) -> Option<Route> {
        let conn = self.conn.as_ref()?;
        let links = if self.trees.is_empty() {
            Links::None
        } else {
            Links::Visible
        };
        if bootstrap::is_bootstrap_mode(&conn.schema, &conn.info.permissions, links) {
            return Some(Route::new(None, RouteTarget::Bootstrap));
        }
        self.landing(0)
    }

    fn landing(&self, layout: usize) -> Option<Route> {
        let conn = self.conn.as_ref()?;
        let tree = self.trees.get(layout)?;
        let view = self
            .prefs
            .last_view(&tree.name)
            .filter(|view| tree.find(view).is_some_and(|node| !node.locked))
            .or_else(|| tree.first_link())?;
        Some(link_route(conn, u8::try_from(layout).ok(), view))
    }

    fn store_warnings(&mut self, now: Instant) {
        let Some(conn) = &self.conn else {
            return;
        };
        if self.flash.deadline().is_some_and(|at| at > now) {
            return;
        }
        let mut text = String::new();
        for warning in conn.credential.take_store_warnings() {
            if !text.is_empty() {
                text.push_str("; ");
            }
            text.push_str(warning.message());
        }
        if !text.is_empty() {
            self.flash.set(Level::Warn, text, now);
        }
    }

    fn sign_out(&mut self, fx: &mut Effects) {
        let session = self
            .conn
            .as_ref()
            .and_then(|conn| conn.credential.session());
        self.logout("Signed out.");
        if let Some(session) = session {
            let target = AuthTarget {
                url: session.server.clone(),
                tls: self.launch.tls,
                auth_dir: self.launch.auth_dir.clone(),
                writer_id: self.launch.writer_id,
            };
            let ticket = self.slots.start(Slot::Auth);
            fx.spawn(
                ticket,
                Job::Auth(AuthJob::Revoke {
                    target,
                    session: Box::new(session),
                }),
            );
        }
    }

    fn expire(&mut self) {
        let route = self.router.current().cloned();
        let notice = match self.conn.as_ref().map(|conn| &*conn.credential) {
            Some(Credential::Static(_)) => CREDENTIALS_REJECTED,
            _ => SESSION_EXPIRED,
        };
        self.logout(notice);
        self.resume = route;
    }

    fn logout(&mut self, notice: &str) {
        self.slots.bump_all();
        match &mut self.screen {
            Screen::Browser(state) => browser::cancel_running(state),
            Screen::Tools(state) => tools::cancel_running(state),
            _ => {}
        }
        let Some(conn) = self.conn.take() else {
            return;
        };
        self.resume = None;
        self.trees.clear();
        self.index = PaletteIndex::default();
        self.overlays.clear();
        self.router.reset();
        self.focus = Focus::Content;
        self.screen = Screen::Login(Box::new(login::State::signed_out(
            &self.launch,
            notice,
            &conn.url,
            &conn.account,
        )));
    }

    fn enter(
        &mut self,
        route: Route,
        restored: Option<Box<browser::State>>,
        fx: &mut Effects,
        now: Instant,
    ) {
        self.slots
            .bump_where(|slot| !matches!(slot, Slot::Session | Slot::Auth));
        self.focus = Focus::Content;
        if let (Some(layout), Some(link)) = (route.layout, route.link()) {
            let layout = usize::from(layout);
            self.sidebar.select_view(&self.trees, layout, &link);
            if !matches!(route.target, RouteTarget::Upsell(_))
                && let Some(tree) = self.trees.get(layout)
                && self.prefs.remember(&tree.name, &link)
            {
                self.prefs_dirty = true;
            }
        }
        self.screen = match &route.target {
            RouteTarget::Log => match &self.log {
                Some(log) => Screen::Log(log_view::State::new(log.clone(), now)),
                None => Screen::Message(message::State::new(
                    "HTTP log",
                    "Start the console with --debug to record HTTP requests.",
                )),
            },
            RouteTarget::Unknown(view) => Screen::Message(message::State::unknown(view)),
            RouteTarget::Upsell(view) => {
                let label = route
                    .layout
                    .and_then(|layout| self.trees.get(usize::from(layout)))
                    .and_then(|tree| tree.find(view))
                    .map_or(&**view, |node| &*node.label);
                Screen::Message(message::State::upsell(label))
            }
            RouteTarget::Dashboard(id) => match self.conn.as_mut() {
                Some(conn) => {
                    let mut state = dashboard::State::new(
                        conn,
                        id.as_deref(),
                        self.prefs.period,
                        UtcTimestamp::from_system(SystemTime::now()),
                    );
                    let mut cx = Cx {
                        fx,
                        slots: &mut self.slots,
                        flash: &mut self.flash,
                        overlays: &mut self.overlays,
                        router: &mut self.router,
                        conn,
                        focus: &mut self.focus,
                        now,
                    };
                    state.start(&mut cx);
                    Screen::Dashboard(Box::new(state))
                }
                None => Screen::Message(message::State::unknown("the dashboard")),
            },
            RouteTarget::LiveDelivery => Screen::Delivery(Box::default()),
            RouteTarget::LiveTracing => match &self.conn {
                Some(conn) => Screen::Tracing(Box::new(tracing::State::new(conn))),
                None => Screen::Message(message::State::unknown("live tracing")),
            },
            RouteTarget::List(view) | RouteTarget::Query { view, .. } => match self.conn.as_mut() {
                Some(conn) => {
                    let preset = match &route.target {
                        RouteTarget::Query { field, value, .. } => Some((&**field, &**value)),
                        _ => None,
                    };
                    let mut cx = Cx {
                        fx,
                        slots: &mut self.slots,
                        flash: &mut self.flash,
                        overlays: &mut self.overlays,
                        router: &mut self.router,
                        conn,
                        focus: &mut self.focus,
                        now,
                    };
                    match restored {
                        Some(mut state) => {
                            state.resume(&mut cx);
                            Screen::Browser(state)
                        }
                        None => {
                            let mut state =
                                browser::State::new(cx.conn, route.layout, view, preset);
                            state.start(&mut cx);
                            Screen::Browser(Box::new(state))
                        }
                    }
                }
                None => Screen::Message(message::State::unknown(view)),
            },
            RouteTarget::Object {
                view,
                id: ObjectRef::Id(id),
            } if self
                .conn
                .as_ref()
                .is_some_and(|conn| trace_detail::is_trace(&conn.schema, view)) =>
            {
                match self.conn.as_mut() {
                    Some(conn) => {
                        let mut state = trace_detail::State::new(conn, id);
                        let mut cx = Cx {
                            fx,
                            slots: &mut self.slots,
                            flash: &mut self.flash,
                            overlays: &mut self.overlays,
                            router: &mut self.router,
                            conn,
                            focus: &mut self.focus,
                            now,
                        };
                        state.start(&mut cx);
                        Screen::TraceDetail(Box::new(state))
                    }
                    None => Screen::Message(message::State::unknown(view)),
                }
            }
            RouteTarget::New(view)
            | RouteTarget::Singleton(view)
            | RouteTarget::Object { view, .. } => match self.conn.as_mut() {
                Some(conn) => {
                    let target = match &route.target {
                        RouteTarget::New(_) => form::Target::Create,
                        RouteTarget::Object {
                            id: ObjectRef::Id(id),
                            ..
                        } => form::Target::Edit(id.clone()),
                        _ => form::Target::Singleton,
                    };
                    let detail = match &target {
                        form::Target::Edit(id) => Some(&**id),
                        form::Target::Singleton => Some(form::SINGLETON_ID),
                        form::Target::Create | form::Target::Action(_) => None,
                    }
                    .filter(|_| !allowed(&conn.schema, &conn.info.permissions, view, Op::Update));
                    let mut cx = Cx {
                        fx,
                        slots: &mut self.slots,
                        flash: &mut self.flash,
                        overlays: &mut self.overlays,
                        router: &mut self.router,
                        conn,
                        focus: &mut self.focus,
                        now,
                    };
                    match detail {
                        Some(id) => {
                            let mut state = detail::State::new(cx.conn, view, id);
                            state.start(&mut cx);
                            Screen::Detail(Box::new(state))
                        }
                        None => {
                            let mut state = form::State::new(cx.conn, view, target);
                            state.start(&mut cx);
                            Screen::Form(Box::new(state))
                        }
                    }
                }
                None => Screen::Message(message::State::unknown(view)),
            },
            RouteTarget::Action(view) => match self.conn.as_ref() {
                Some(conn) => Screen::Action(Box::new(action::State::new(conn, view))),
                None => Screen::Message(message::State::unknown("actions")),
            },
            RouteTarget::Tools => Screen::Tools(Box::default()),
            RouteTarget::Bootstrap => match self.conn.as_mut() {
                Some(conn) => {
                    let mut state = bootstrap::State::new();
                    let mut cx = Cx {
                        fx,
                        slots: &mut self.slots,
                        flash: &mut self.flash,
                        overlays: &mut self.overlays,
                        router: &mut self.router,
                        conn,
                        focus: &mut self.focus,
                        now,
                    };
                    state.start(&mut cx);
                    Screen::Bootstrap(Box::new(state))
                }
                None => Screen::Message(message::State::unknown("the setup wizard")),
            },
        };
    }

    fn key(&mut self, key: KeyEvent, fx: &mut Effects, now: Instant) {
        if keymap::matches(&[keymap::ctrl('c')], key) {
            self.quit = true;
            return;
        }
        let ctx = self.context();
        let help = keymap::matches(&[keymap::code(KeyCode::F(1))], key)
            || (keymap::matches(&[keymap::key('?')], key) && !ctx.types_question_mark());
        if help && !matches!(self.overlays.last(), Some(Overlay::Help(_))) {
            self.overlays.push(Overlay::Help(ctx));
            return;
        }
        match self.overlays.last_mut() {
            Some(Overlay::Confirm(dialog, action)) => {
                let action = *action;
                match dialog.handle(key) {
                    dialog::Outcome::Pending => {}
                    dialog::Outcome::Cancel => {
                        self.overlays.pop();
                    }
                    dialog::Outcome::Key(_) => {
                        self.overlays.pop();
                        match action {
                            Confirmable::Logout => self.sign_out(fx),
                        }
                    }
                }
                return;
            }
            Some(Overlay::Help(_)) => {
                self.overlays.pop();
                return;
            }
            Some(Overlay::Accounts(picker)) => {
                match picker.handle(key) {
                    picker::Outcome::Cancel => {
                        self.overlays.pop();
                    }
                    picker::Outcome::Chosen(index) => {
                        self.overlays.pop();
                        self.switch_account(index, now);
                    }
                    picker::Outcome::Pending
                    | picker::Outcome::QueryChanged
                    | picker::Outcome::ChosenMany(_) => {}
                }
                return;
            }
            Some(Overlay::Palette(state)) => {
                let action = keymap::resolve(palette::KEYS, key);
                match state.handle(&self.index, action, key) {
                    palette::Outcome::Stay => {}
                    palette::Outcome::Close => {
                        self.overlays.pop();
                    }
                    palette::Outcome::Open(target) => {
                        self.overlays.pop();
                        self.open_target(target, now);
                    }
                }
                return;
            }
            None => {}
        }
        if self.focus == Focus::Sidebar && self.conn.is_some() {
            if let Some(action) = keymap::resolve(sidebar::KEYS, key) {
                match self.sidebar.handle(&self.trees, action) {
                    sidebar::Outcome::Stay => {}
                    sidebar::Outcome::Leave => self.focus = Focus::Content,
                    sidebar::Outcome::Open { layout, view } => {
                        if let Some(conn) = &self.conn {
                            let route = link_route(conn, u8::try_from(layout).ok(), &view);
                            self.navigate(route);
                        }
                    }
                    sidebar::Outcome::Switch(layout) => {
                        if let Some(route) = self.landing(layout) {
                            self.navigate(route);
                            if let Some(route) = self.router.take_pending() {
                                self.parked.clear();
                                self.enter(route, None, fx, now);
                                self.focus = Focus::Sidebar;
                            }
                        }
                    }
                }
                return;
            }
        } else if self.screen_key(key, fx, now) {
            return;
        }
        if let Some(action) = keymap::resolve(global::KEYS, key) {
            self.global(action, now);
        }
    }

    fn screen_key(&mut self, key: KeyEvent, fx: &mut Effects, now: Instant) -> bool {
        let ctx = self.screen_context();
        if let Screen::Login(state) = &mut self.screen {
            let mut cx = LoginCx {
                fx,
                slots: &mut self.slots,
                flash: &mut self.flash,
                launch: &self.launch,
                connected: &mut self.conn,
                now,
            };
            let table: &[Binding<login::Action>] = login::keys(state);
            return screen_key!(login, state, table, &mut cx, key, ctx);
        }
        let Some(conn) = self.conn.as_mut() else {
            return false;
        };
        let mut cx = Cx {
            fx,
            slots: &mut self.slots,
            flash: &mut self.flash,
            overlays: &mut self.overlays,
            router: &mut self.router,
            conn,
            focus: &mut self.focus,
            now,
        };
        match &mut self.screen {
            Screen::Login(_) => false,
            Screen::Message(state) => {
                let table = message::keys(state);
                screen_key!(message, state, table, &mut cx, key, ctx)
            }
            Screen::Log(state) => screen_key!(log_view, state, log_view::KEYS, &mut cx, key, ctx),
            Screen::Browser(state) => browser::handle_key(state, &mut cx, key),
            Screen::Form(state) => form::handle_key(state, &mut cx, key),
            Screen::Action(state) => action::handle_key(state, &mut cx, key),
            Screen::Tools(state) => tools::handle_key(state, &mut cx, key),
            Screen::Bootstrap(state) => bootstrap::handle_key(state, &mut cx, key),
            Screen::Detail(state) => detail::handle_key(state, &mut cx, key),
            Screen::Tracing(state) => tracing::handle_key(state, &mut cx, key),
            Screen::Delivery(state) => delivery::handle_key(state, &mut cx, key),
            Screen::TraceDetail(state) => trace_detail::handle_key(state, &mut cx, key),
            Screen::Dashboard(state) => dashboard::handle_key(state, &mut cx, key),
        }
    }

    fn screen_msg(&mut self, msg: Passive<'_>, fx: &mut Effects, now: Instant) {
        if let Screen::Login(state) = &mut self.screen {
            let mut cx = LoginCx {
                fx,
                slots: &mut self.slots,
                flash: &mut self.flash,
                launch: &self.launch,
                connected: &mut self.conn,
                now,
            };
            login::update(state, &mut cx, msg.into_msg());
            return;
        }
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        let mut cx = Cx {
            fx,
            slots: &mut self.slots,
            flash: &mut self.flash,
            overlays: &mut self.overlays,
            router: &mut self.router,
            conn,
            focus: &mut self.focus,
            now,
        };
        match &mut self.screen {
            Screen::Login(_) => {}
            Screen::Message(state) => message::update(state, &mut cx, msg.into_msg()),
            Screen::Log(state) => log_view::update(state, &mut cx, msg.into_msg()),
            Screen::Browser(state) => browser::update(state, &mut cx, msg.into_msg()),
            Screen::Form(state) => form::update(state, &mut cx, msg.into_msg()),
            Screen::Action(state) => action::update(state, &mut cx, msg.into_msg()),
            Screen::Tools(state) => tools::update(state, &mut cx, msg.into_msg()),
            Screen::Bootstrap(state) => bootstrap::update(state, &mut cx, msg.into_msg()),
            Screen::Detail(state) => detail::update(state, &mut cx, msg.into_msg()),
            Screen::Tracing(state) => tracing::update(state, &mut cx, msg.into_msg()),
            Screen::Delivery(state) => delivery::update(state, &mut cx, msg.into_msg()),
            Screen::TraceDetail(state) => trace_detail::update(state, &mut cx, msg.into_msg()),
            Screen::Dashboard(state) => dashboard::update(state, &mut cx, msg.into_msg()),
        }
    }

    fn paste(&mut self, text: &str, fx: &mut Effects, now: Instant) {
        match self.overlays.last_mut() {
            Some(Overlay::Palette(state)) => {
                state.paste(&self.index, text);
                return;
            }
            Some(Overlay::Accounts(picker)) => {
                picker.paste(text);
                return;
            }
            _ => {}
        }
        if self.overlays.is_empty() && self.focus == Focus::Content {
            self.screen_msg(Passive::Paste(text), fx, now);
        }
    }

    fn global(&mut self, action: GlobalAction, now: Instant) {
        match action {
            GlobalAction::Quit => self.quit = true,
            GlobalAction::Help => {
                let ctx = self.context();
                self.overlays.push(Overlay::Help(ctx));
            }
            GlobalAction::Palette if self.conn.is_some() => {
                self.overlays
                    .push(Overlay::Palette(PaletteState::new(&self.index)));
            }
            GlobalAction::Palette => {}
            GlobalAction::ToggleSidebar if self.conn.is_some() => {
                let text = self.sidebar.toggle_mode();
                self.prefs.sidebar = Some(self.sidebar.mode);
                self.prefs_dirty = true;
                self.flash.set(Level::Info, text, now);
            }
            GlobalAction::ToggleSidebar => {}
            GlobalAction::FocusSidebar if self.conn.is_some() => self.focus = Focus::Sidebar,
            GlobalAction::FocusSidebar => {}
            GlobalAction::ToggleTheme => self.cycle_theme(now),
            GlobalAction::Reload if self.conn.is_some() => {
                let reload = match &mut self.screen {
                    Screen::Form(state) => form::guard_reload(state),
                    Screen::Browser(state) => browser::guard_reload(state),
                    Screen::Tools(state) => tools::guard_reload(state),
                    _ => true,
                };
                if reload {
                    self.router.reload();
                }
            }
            GlobalAction::Reload => {}
            GlobalAction::HttpLog if self.conn.is_some() => {
                self.navigate(Route::new(None, RouteTarget::Log));
            }
            GlobalAction::HttpLog => {}
            GlobalAction::Accounts => self.open_accounts(now),
            GlobalAction::Logout if self.conn.is_some() => {
                let dialog = Dialog::confirm(
                    "Sign out",
                    Risk::Low,
                    vec![Line::from(
                        "Sign out of this server and return to the login screen?",
                    )],
                    CONFIRM_LOGOUT,
                );
                self.overlays
                    .push(Overlay::Confirm(Box::new(dialog), Confirmable::Logout));
            }
            GlobalAction::Logout => {}
            GlobalAction::Back => {
                if self.focus == Focus::Sidebar {
                    self.focus = Focus::Content;
                } else if !self.router.back() {
                    self.focus = Focus::Sidebar;
                }
            }
        }
    }

    fn open_accounts(&mut self, now: Instant) {
        let Some(conn) = &self.conn else {
            return;
        };
        if let Screen::Form(state) = &self.screen
            && form::is_dirty(state)
        {
            self.flash.set(Level::Warn, SWITCH_DIRTY, now);
            return;
        }
        if let Screen::Browser(state) = &self.screen
            && browser::is_running(state)
        {
            self.flash.set(Level::Warn, browser::RUNNING, now);
            return;
        }
        if let Screen::Tools(state) = &self.screen
            && tools::is_running(state)
        {
            self.flash.set(Level::Warn, tools::RUNNING, now);
            return;
        }
        let accounts = &conn.client.session().accounts;
        if accounts.is_empty() {
            self.flash.set(Level::Warn, NO_ACCOUNTS, now);
            return;
        }
        let items = accounts
            .iter()
            .map(|account| {
                let kind = if account.is_personal {
                    "personal"
                } else {
                    "shared"
                };
                PickerItem::new(&*account.id, &*account.name, kind)
            })
            .collect();
        let picker = Picker::new(SWITCH_ACCOUNT, items, Selection::Single)
            .with_current(conn.active_account.as_deref());
        self.overlays.push(Overlay::Accounts(Box::new(picker)));
    }

    fn switch_account(&mut self, index: usize, now: Instant) {
        let Some(conn) = &mut self.conn else {
            return;
        };
        let Some(account) = conn.client.session().accounts.get(index) else {
            return;
        };
        conn.active_account = Some(account.id.clone());
        conn.labels = LabelCache::default();
        let mut text = String::from("account: ");
        text.push_str(&account.name);
        self.flash.set(Level::Info, text, now);
        self.rebuild_navigation();
        match self.router.current() {
            Some(route) if self.reachable(route) => self.router.reload(),
            _ => {
                if let Some(route) = self.start_route() {
                    self.router.replace(route);
                }
            }
        }
    }

    fn navigate(&mut self, route: Route) {
        let route = match &mut self.screen {
            Screen::Form(state) => form::guard_leave(state, route),
            Screen::Browser(state) => browser::guard_leave(state, route),
            Screen::Tools(state) => tools::guard_leave(state, route),
            _ => Some(route),
        };
        match route {
            Some(route) => self.router.jump(route),
            None => self.focus = Focus::Content,
        }
    }

    fn cycle_theme(&mut self, now: Instant) {
        self.theme = Theme::new(self.theme.palette.next());
        self.prefs.theme = Some(self.theme.palette);
        self.prefs_dirty = true;
        let mut text = String::from("theme: ");
        text.push_str(self.theme.palette.label());
        self.flash.set(Level::Info, text, now);
    }

    fn open_target(&mut self, target: PaletteTarget, now: Instant) {
        match target {
            PaletteTarget::Route(route) => self.navigate(route),
            PaletteTarget::Command(command) => match command {
                Command::Tools => self.navigate(Route::new(None, RouteTarget::Tools)),
                Command::Accounts => self.open_accounts(now),
                Command::TryEnterprise => {
                    self.navigate(Route::new(None, RouteTarget::Upsell(Box::default())))
                }
                Command::HttpLog => self.global(GlobalAction::HttpLog, now),
                Command::Logout => self.global(GlobalAction::Logout, now),
                Command::Theme => self.cycle_theme(now),
                Command::Sidebar => self.global(GlobalAction::ToggleSidebar, now),
                Command::Quit => self.quit = true,
            },
        }
    }

    pub fn render(&mut self, frame: &mut Frame, now: Instant) {
        let tick = self.tick(now);
        let area = frame.area();
        if self.theme.bg != ratatui::style::Color::Reset {
            frame.render_widget(Block::new().style(self.theme.base()), area);
        }
        if let Screen::Login(state) = &mut self.screen {
            login::render(state, frame, &self.theme, &self.flash, tick);
            self.render_overlays(frame);
            return;
        }
        let Some(conn) = self.conn.as_ref() else {
            return;
        };
        let [head, body, foot] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(area);
        self.crumbs.clear();
        let layout = self
            .router
            .current()
            .and_then(|r| r.layout)
            .and_then(|l| self.trees.get(usize::from(l)));
        if let Some(tree) = layout {
            self.crumbs.push(&tree.name);
        }
        match &self.screen {
            Screen::Login(_) => {}
            Screen::Message(state) => message::crumbs(state, &mut self.crumbs),
            Screen::Log(state) => log_view::crumbs(state, &mut self.crumbs),
            Screen::Browser(state) => browser::crumbs(state, &mut self.crumbs),
            Screen::Form(state) => form::crumbs(state, &mut self.crumbs),
            Screen::Action(state) => action::crumbs(state, &mut self.crumbs),
            Screen::Tools(state) => tools::crumbs(state, &mut self.crumbs),
            Screen::Bootstrap(state) => bootstrap::crumbs(state, &mut self.crumbs),
            Screen::Detail(state) => detail::crumbs(state, &mut self.crumbs),
            Screen::Tracing(state) => tracing::crumbs(state, &mut self.crumbs),
            Screen::Delivery(state) => delivery::crumbs(state, &mut self.crumbs),
            Screen::TraceDetail(state) => trace_detail::crumbs(state, &mut self.crumbs),
            Screen::Dashboard(state) => dashboard::crumbs(state, &mut self.crumbs),
        }
        let info = HeaderInfo {
            server: &conn.server,
            account: &conn.account,
            edition: conn.info.edition,
            health: self.health,
        };
        header(frame, head, &self.theme, &info, &self.crumbs);
        let sidebar_focus = self.focus == Focus::Sidebar;
        let sidebar_shown = self.trees.is_empty() || self.sidebar.shown(sidebar_focus, area.width);
        let content = if sidebar_shown && !self.trees.is_empty() {
            let [side, rest] =
                Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Fill(1)])
                    .areas(body);
            let current = self.router.current().and_then(Route::link);
            self.sidebar.render(
                frame,
                side,
                &self.theme,
                &self.trees,
                current.as_deref(),
                sidebar_focus,
            );
            rest
        } else {
            body
        };
        let cx = ViewCx {
            theme: &self.theme,
            conn,
            tick,
            now,
            focused: !sidebar_focus,
        };
        render_screen(&mut self.screen, frame, content, &cx);
        let ctx = self.context();
        let info = FooterInfo {
            ctx,
            available: &|index| self.hint_available(ctx, index),
            sidebar_hidden: !sidebar_shown,
        };
        footer(frame, foot, &self.theme, &info, self.flash.get());
        self.render_overlays(frame);
    }

    fn hint_available(&self, ctx: Context, index: usize) -> bool {
        match (&self.screen, ctx) {
            (Screen::Browser(state), Context::Browser) => browser::KEYS
                .get(index)
                .is_none_or(|binding| browser::available(state, binding.action)),
            (Screen::Delivery(state), Context::Delivery) => delivery::KEYS
                .get(index)
                .is_none_or(|binding| delivery::available(state, binding.action)),
            (Screen::Tracing(state), Context::Tracing) => tracing::SESSION_KEYS
                .get(index)
                .is_none_or(|binding| tracing::available(state, binding.action)),
            _ => true,
        }
    }

    fn render_overlays(&mut self, frame: &mut Frame) {
        for overlay in &mut self.overlays {
            match overlay {
                Overlay::Palette(state) => state.render(frame, &self.theme, &self.index),
                Overlay::Help(ctx) => help_overlay(frame, &self.theme, *ctx),
                Overlay::Confirm(dialog, _) => dialog.render(frame, frame.area(), &self.theme),
                Overlay::Accounts(picker) => picker.render(frame, frame.area(), &self.theme),
            }
        }
    }
}

fn render_screen(screen: &mut Screen, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    match screen {
        Screen::Login(_) => {}
        Screen::Message(state) => message::render(state, frame, area, cx),
        Screen::Log(state) => log_view::render(state, frame, area, cx),
        Screen::Browser(state) => browser::render(state, frame, area, cx),
        Screen::Form(state) => form::render(state, frame, area, cx),
        Screen::Action(state) => action::render(state, frame, area, cx),
        Screen::Tools(state) => tools::render(state, frame, area, cx),
        Screen::Bootstrap(state) => bootstrap::render(state, frame, area, cx),
        Screen::Detail(state) => detail::render(state, frame, area, cx),
        Screen::Tracing(state) => tracing::render(state, frame, area, cx),
        Screen::Delivery(state) => delivery::render(state, frame, area, cx),
        Screen::TraceDetail(state) => trace_detail::render(state, frame, area, cx),
        Screen::Dashboard(state) => dashboard::render(state, frame, area, cx),
    }
}

pub enum Passive<'a> {
    Paste(&'a str),
    Done(JobOutput),
    Stream(StreamEvent),
    Tick(Instant),
    Resumed(Option<&'a io::Error>),
    Progress(JobProgress),
}

impl<'a> Passive<'a> {
    fn into_msg<A>(self) -> ScreenMsg<'a, A> {
        match self {
            Passive::Paste(text) => ScreenMsg::Paste(text),
            Passive::Done(output) => ScreenMsg::Done(output),
            Passive::Stream(event) => ScreenMsg::Stream(event),
            Passive::Tick(now) => ScreenMsg::Tick(now),
            Passive::Resumed(error) => ScreenMsg::Resumed(error),
            Passive::Progress(progress) => ScreenMsg::Progress(progress),
        }
    }
}

pub fn link_route(conn: &Connected, layout: Option<u8>, view: &str) -> Route {
    let target = match conn.access(view) {
        Access::Locked => RouteTarget::Upsell(view.into()),
        Access::Open | Access::Hidden => RouteTarget::for_view(&conn.schema, view),
    };
    Route::new(layout, target)
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/tui/app/tests.rs"]
mod tests;
