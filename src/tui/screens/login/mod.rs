/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::auth::device::DeviceGrant;
use crate::auth::login::{AuthPreference, BasicReason, is_dcr_refusal};
use crate::auth::store::StoredSession;
use crate::cli::args::AuthMethod;
use crate::error::Error;
use crate::jmap::auth::Auth;
use crate::tui::app::{LoginCx, ScreenMsg};
use crate::tui::exec::{Cancel, Slot};
use crate::tui::job::auth::{AuthJob, AuthOutput, AuthTarget, Planned};
use crate::tui::job::session::{ConnectRequest, Link, Login, SessionJob, SessionOutput};
use crate::tui::keymap::{Binding, Context, code, ctrl, key, keys, shifted};
use crate::tui::msg::{Effect, HttpUrl, Job, JobOutput};
use crate::tui::session::Connected;
use crate::tui::theme::Level;
use crate::tui::widgets::input::Input;
use crate::tui::widgets::spinner;
use crate::tui::{Launch, StaticAuth};
use ratatui::crossterm::event::KeyCode;
use std::time::{Duration, Instant, SystemTime};

mod view;

pub use view::render;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Confirm,
    NextField,
    PrevField,
    Back,
    Open,
    Copy,
    Basic,
    Device,
}

const MFA_HINT: &str =
    ". Two-factor accounts sign in with the device code (^b), an app password or an API key.";

keys! { pub INPUT_KEYS, pub INPUT_HINTS: Action;
    "⏎" Confirm "continue" true [code(KeyCode::Enter)],
    "tab" NextField "next field" true [code(KeyCode::Tab), code(KeyCode::Down)],
    "S-tab" PrevField "previous field" false [shifted(KeyCode::BackTab), code(KeyCode::Up)],
    "esc" Back "clear error" false [code(KeyCode::Esc)],
    "^c" Back "quit" true [ctrl('c')],
}

keys! { pub BASIC_KEYS, pub BASIC_HINTS: Action;
    "⏎" Confirm "sign in" true [code(KeyCode::Enter)],
    "tab" NextField "next field" true [code(KeyCode::Tab), code(KeyCode::Down)],
    "S-tab" PrevField "previous field" false [shifted(KeyCode::BackTab), code(KeyCode::Up)],
    "^b" Device "try device code again" true [ctrl('b')],
    "esc" Back "back" true [code(KeyCode::Esc)],
}

keys! { pub DEVICE_KEYS, pub DEVICE_HINTS: Action;
    "o" Open "open in browser" true [key('o')],
    "c" Copy "copy code" true [key('c')],
    "b" Basic "use a password" true [key('b')],
    "esc" Back "cancel" true [code(KeyCode::Esc)],
}

keys! { pub KEYS, pub HINTS: Action;
    "esc" Back "cancel" true [code(KeyCode::Esc)],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Url,
    Account,
    Password,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Stored,
    Discover,
    DeviceCode,
}

pub enum Stage {
    Input,
    Discovering { step: Step },
    Device(Box<DeviceView>),
    Basic,
    Connecting,
}

pub struct DeviceView {
    pub grant: DeviceGrant,
    pub issued: Instant,
    pub lifetime: Duration,
    cancel: Cancel,
}

pub struct State {
    pub stage: Stage,
    pub field: Field,
    pub url: Input,
    pub account: Input,
    pub password: Input,
    api_key: Option<String>,
    reason: Option<BasicReason>,
    error: Option<Box<str>>,
    notice: Option<Box<str>>,
    signing_in: Box<str>,
    logo: Option<view::Logo>,
}

impl State {
    pub fn new(launch: &Launch) -> Self {
        let (account, password, api_key) = match &launch.auth {
            StaticAuth::None => ("", "", None),
            StaticAuth::ApiKey(key) => ("", "", Some(key.clone())),
            StaticAuth::User { user, password } => {
                (user.as_str(), password.as_deref().unwrap_or_default(), None)
            }
        };
        let url = launch.url.as_deref().unwrap_or_default();
        let field = if url.is_empty() {
            Field::Url
        } else {
            Field::Account
        };
        State {
            stage: Stage::Input,
            field,
            url: Input::new(url),
            account: Input::new(account),
            password: Input::masked(password),
            api_key,
            reason: None,
            error: None,
            notice: None,
            signing_in: Box::default(),
            logo: None,
        }
    }

    pub fn signed_out(launch: &Launch, notice: &str, url: &str, account: &str) -> Self {
        let mut state = State::new(launch);
        state.url.set(url);
        if state.api_key.is_none() {
            state.account.set(account);
        }
        state.password.clear();
        state.field = Field::Account;
        state.notice = Some(notice.into());
        state
    }

    fn has_static_credentials(&self) -> bool {
        self.api_key.is_some() || (!self.account.is_empty() && !self.password.is_empty())
    }

    fn fields(&self) -> &'static [Field] {
        match self.stage {
            Stage::Basic => &[Field::Account, Field::Password],
            _ => &[Field::Url, Field::Account],
        }
    }

    fn input(&mut self) -> &mut Input {
        match self.field {
            Field::Url => &mut self.url,
            Field::Account => &mut self.account,
            Field::Password => &mut self.password,
        }
    }

    fn cycle(&mut self, forward: bool) {
        let fields = self.fields();
        let pos = fields.iter().position(|f| *f == self.field).unwrap_or(0);
        let len = fields.len();
        let next = if forward { pos + 1 } else { pos + len - 1 } % len;
        self.field = fields.get(next).copied().unwrap_or(Field::Url);
    }

    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        match self.stage {
            Stage::Connecting | Stage::Discovering { .. } | Stage::Device(_) => {
                Some(now + spinner::FRAME)
            }
            Stage::Input | Stage::Basic => None,
        }
    }

    fn fail(&mut self, error: &Error) {
        self.cancel_device();
        let mut text = error.to_string();
        if matches!(error, Error::MfaRequired) {
            text.push_str(MFA_HINT);
        }
        self.error = Some(text.into_boxed_str());
        self.stage = if self.reason.is_some() {
            Stage::Basic
        } else {
            Stage::Input
        };
    }

    fn cancel_device(&mut self) {
        if let Stage::Device(view) = &self.stage {
            view.cancel.cancel();
        }
    }

    fn enter_basic(&mut self, reason: BasicReason) {
        self.cancel_device();
        self.reason = Some(reason);
        self.stage = Stage::Basic;
        self.field = if self.account.is_empty() {
            Field::Account
        } else {
            Field::Password
        };
    }
}

fn normalised_url(raw: &str) -> Result<String, &'static str> {
    let url = raw.trim().trim_end_matches('/');
    if url.starts_with("https://") || url.starts_with("http://") {
        Ok(url.to_string())
    } else if url.is_empty() {
        Err("Enter the server URL, for example https://mail.example.org.")
    } else {
        Err("The server URL must start with https:// or http://.")
    }
}

fn preference(launch: &Launch) -> AuthPreference {
    match launch.pref {
        Some(AuthMethod::Device) => AuthPreference::Device,
        Some(AuthMethod::Basic) => AuthPreference::Basic,
        None => AuthPreference::Auto,
    }
}

fn target(state: &mut State, cx: &LoginCx<'_>) -> Option<AuthTarget> {
    match normalised_url(state.url.value()) {
        Ok(url) => Some(AuthTarget {
            url,
            tls: cx.launch.tls,
            auth_dir: cx.launch.auth_dir.clone(),
            writer_id: cx.launch.writer_id,
        }),
        Err(reason) => {
            state.error = Some(reason.into());
            state.field = Field::Url;
            state.stage = Stage::Input;
            None
        }
    }
}

fn account(state: &State) -> Option<String> {
    let account = state.account.value().trim();
    (!account.is_empty()).then(|| account.to_string())
}

pub fn start(state: &mut State, cx: &mut LoginCx<'_>) {
    if state.has_static_credentials() {
        connect_static(state, cx);
    } else if !state.url.is_empty() && preference(cx.launch) != AuthPreference::Basic {
        let Some(target) = target(state, cx) else {
            return;
        };
        let job = AuthJob::Stored {
            target,
            account: account(state),
        };
        spawn_auth(state, cx, job, Step::Stored);
    }
}

fn discover(state: &mut State, cx: &mut LoginCx<'_>, pref: AuthPreference) {
    let Some(target) = target(state, cx) else {
        return;
    };
    let job = AuthJob::Plan {
        target,
        account: account(state),
        pref,
        client_id: cx.launch.client_id.clone(),
    };
    spawn_auth(state, cx, job, Step::Discover);
}

fn spawn_auth(state: &mut State, cx: &mut LoginCx<'_>, job: AuthJob, step: Step) {
    state.error = None;
    state.stage = Stage::Discovering { step };
    let ticket = cx.slots.start(Slot::Auth);
    cx.fx.spawn(ticket, Job::Auth(job));
}

fn connect_static(state: &mut State, cx: &mut LoginCx<'_>) {
    let auth = match &state.api_key {
        Some(token) => Auth::Bearer {
            token: token.clone(),
        },
        None if state.account.is_empty() => {
            state.error = Some("Enter the account name.".into());
            state.field = Field::Account;
            return;
        }
        None => Auth::Basic {
            user: state.account.value().to_string(),
            password: state.password.value().to_string(),
        },
    };
    let label = match state.api_key {
        Some(_) => "API key",
        None => state.account.value(),
    };
    state.signing_in = label.into();
    connect(state, cx, Login::Static(auth));
}

fn connect_session(state: &mut State, cx: &mut LoginCx<'_>, session: Box<StoredSession>) {
    state.signing_in = session.account.as_str().into();
    connect(state, cx, Login::Session(session));
}

fn connect(state: &mut State, cx: &mut LoginCx<'_>, login: Login) {
    let Some(target) = target(state, cx) else {
        return;
    };
    state.error = None;
    state.notice = None;
    state.stage = Stage::Connecting;
    let ticket = cx.slots.start(Slot::Session);
    cx.fx.spawn(
        ticket,
        Job::Session(SessionJob::Connect(Box::new(ConnectRequest {
            url: target.url,
            login,
            auth_dir: target.auth_dir,
            tls: target.tls,
            cache_dir: cx.launch.cache_dir.clone(),
            writer_id: target.writer_id,
        }))),
    );
}

fn device_wait(state: &mut State, cx: &mut LoginCx<'_>, output: AuthOutput) {
    let AuthOutput::DeviceStarted(result) = output else {
        return;
    };
    let (plan, flow) = match result {
        Ok(started) => started,
        Err(error) if is_dcr_refusal(&error) => {
            state.enter_basic(BasicReason::DcrRefused(error.to_string()));
            return;
        }
        Err(error) => return state.fail(&error),
    };
    let Some(target) = target(state, cx) else {
        return;
    };
    let lifetime = flow
        .grant
        .expires_at
        .duration_since(SystemTime::now())
        .unwrap_or_default();
    let cancel = Cancel::default();
    state.stage = Stage::Device(Box::new(DeviceView {
        grant: flow.grant.clone(),
        issued: cx.now,
        lifetime,
        cancel: cancel.clone(),
    }));
    let ticket = cx.slots.start(Slot::Auth);
    cx.fx.spawn(
        ticket,
        Job::Auth(AuthJob::DeviceWait {
            target,
            plan,
            flow,
            account: account(state).unwrap_or_default(),
            cancel,
        }),
    );
}

fn planned(state: &mut State, cx: &mut LoginCx<'_>, result: Result<Planned, Box<Error>>) {
    match result {
        Ok(Planned::Nothing) => state.stage = Stage::Input,
        Ok(Planned::Stored(session)) => connect_session(state, cx, session),
        Ok(Planned::Basic(reason)) => state.enter_basic(reason),
        Ok(Planned::Device(plan)) => {
            let Some(target) = target(state, cx) else {
                return;
            };
            spawn_auth(
                state,
                cx,
                AuthJob::DeviceStart { target, plan },
                Step::DeviceCode,
            );
        }
        Err(error) => state.fail(&error),
    }
}

fn device_action(state: &mut State, cx: &mut LoginCx<'_>, action: Action) {
    let Stage::Device(view) = &state.stage else {
        return;
    };
    match action {
        Action::Open => {
            let grant = &view.grant;
            let uri = grant
                .verification_uri_complete
                .as_deref()
                .unwrap_or(&grant.verification_uri);
            match HttpUrl::parse(uri) {
                Some(url) => cx.fx.push(Effect::OpenUrl(url)),
                None => cx
                    .flash
                    .set(Level::Error, "the server sent a non-web link", cx.now),
            }
        }
        Action::Copy => {
            cx.fx
                .push(Effect::Copy(view.grant.user_code.as_str().into()));
            cx.flash
                .set(Level::Ok, "code copied to the clipboard", cx.now);
        }
        Action::Basic => {
            cx.slots.bump(&Slot::Auth);
            state.enter_basic(BasicReason::Preferred);
        }
        Action::Back => {
            cx.slots.bump(&Slot::Auth);
            state.cancel_device();
            state.stage = Stage::Input;
        }
        _ => {}
    }
}

pub fn context(state: &State) -> Context {
    match state.stage {
        Stage::Input => Context::LoginInput,
        Stage::Basic => Context::LoginBasic,
        Stage::Device(_) => Context::LoginDevice,
        Stage::Discovering { .. } | Stage::Connecting => Context::Login,
    }
}

pub fn keys(state: &State) -> &'static [Binding<Action>] {
    match state.stage {
        Stage::Input => INPUT_KEYS,
        Stage::Basic => BASIC_KEYS,
        Stage::Device(_) => DEVICE_KEYS,
        Stage::Discovering { .. } | Stage::Connecting => KEYS,
    }
}

pub fn update(state: &mut State, cx: &mut LoginCx<'_>, msg: ScreenMsg<'_, Action>) {
    match msg {
        ScreenMsg::Key(action) => key_action(state, cx, action),
        ScreenMsg::Text(key) if matches!(state.stage, Stage::Input | Stage::Basic) => {
            state.input().handle(key);
        }
        ScreenMsg::Paste(text) if matches!(state.stage, Stage::Input | Stage::Basic) => {
            state.input().paste(text)
        }
        ScreenMsg::Done(JobOutput::Auth(AuthOutput::Planned(result))) => planned(state, cx, result),
        ScreenMsg::Done(JobOutput::Auth(output @ AuthOutput::DeviceStarted(_))) => {
            device_wait(state, cx, output)
        }
        ScreenMsg::Done(JobOutput::Auth(AuthOutput::DeviceDone(result))) => match result {
            Ok(Some(session)) => connect_session(state, cx, session),
            Ok(None) => {}
            Err(error) => state.fail(&error),
        },
        ScreenMsg::Done(JobOutput::Session(SessionOutput::Connected(result))) => {
            connected(state, cx, result)
        }
        _ => {}
    }
}

fn key_action(state: &mut State, cx: &mut LoginCx<'_>, action: Action) {
    match (&state.stage, action) {
        (Stage::Input, Action::Confirm) => {
            if state.field == Field::Url {
                if target(state, cx).is_some() {
                    state.error = None;
                    state.field = Field::Account;
                }
            } else if state.has_static_credentials() {
                connect_static(state, cx);
            } else {
                discover(state, cx, preference(cx.launch));
            }
        }
        (Stage::Basic, Action::Confirm) => {
            if state.field == Field::Account || state.password.is_empty() {
                state.field = Field::Password;
            } else {
                connect_static(state, cx);
            }
        }
        (Stage::Input | Stage::Basic, Action::NextField) => state.cycle(true),
        (Stage::Input | Stage::Basic, Action::PrevField) => state.cycle(false),
        (Stage::Input, Action::Back) => state.error = None,
        (Stage::Basic, Action::Back) => {
            state.reason = None;
            state.error = None;
            state.password.clear();
            state.field = Field::Account;
            state.stage = Stage::Input;
        }
        (Stage::Basic, Action::Device) => {
            state.reason = None;
            discover(state, cx, AuthPreference::Device);
        }
        (Stage::Device(_), action) => device_action(state, cx, action),
        (Stage::Discovering { .. }, Action::Back) => {
            cx.slots.bump(&Slot::Auth);
            state.stage = Stage::Input;
        }
        (Stage::Connecting, Action::Back) => {
            cx.slots.bump(&Slot::Session);
            state.stage = if state.reason.is_some() {
                Stage::Basic
            } else {
                Stage::Input
            };
        }
        _ => {}
    }
}

fn connected(state: &mut State, cx: &mut LoginCx<'_>, result: Result<Box<Link>, Box<Error>>) {
    let link = match result {
        Ok(link) => *link,
        Err(error) => {
            if matches!(*error, Error::MfaRequired) {
                state.reason.get_or_insert(BasicReason::Preferred);
            }
            return state.fail(&error);
        }
    };
    state.stage = Stage::Input;
    if let Some(stale) = &link.stale {
        let mut text = String::from("using the cached schema, refreshing it failed: ");
        text.push_str(&stale.to_string());
        cx.flash.set(Level::Warn, text, cx.now);
    }
    *cx.connected = Some(Connected::new(
        link.client,
        link.schema,
        link.account,
        link.credential,
        &link.url,
        &state.signing_in,
    ));
    state.password.clear();
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/login/tests.rs"]
mod tests;
