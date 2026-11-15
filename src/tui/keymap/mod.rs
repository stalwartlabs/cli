/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod global;

use crate::tui::form;
use crate::tui::screens::{
    action, bootstrap, browser, dashboard, delivery, detail, log_view, login, message, tools,
    trace_detail, tracing,
};
use crate::tui::widgets::{dialog, palette, picker, sidebar};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub use global::GlobalAction;

pub type Key = (KeyCode, KeyModifiers);

pub const NONE: KeyModifiers = KeyModifiers::NONE;
pub const CTRL: KeyModifiers = KeyModifiers::CONTROL;
pub const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

pub const fn key(c: char) -> Key {
    (KeyCode::Char(c), NONE)
}

pub const fn upper(c: char) -> Key {
    (KeyCode::Char(c), SHIFT)
}

pub const fn ctrl(c: char) -> Key {
    (KeyCode::Char(c), CTRL)
}

pub const fn code(code: KeyCode) -> Key {
    (code, NONE)
}

pub const fn shifted(code: KeyCode) -> Key {
    (code, SHIFT)
}

pub struct Binding<A> {
    pub keys: &'static [Key],
    pub action: A,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    pub label: &'static str,
    pub desc: &'static str,
    pub footer: bool,
}

macro_rules! keys {
    ($vis:vis $keys:ident, $hvis:vis $hints:ident: $action:ident;
     $($label:literal $act:ident $desc:literal $footer:literal [$($key:expr),+ $(,)?]),* $(,)?) => {
        $vis const $keys: &[$crate::tui::keymap::Binding<$action>] = &[$(
            $crate::tui::keymap::Binding {
                keys: &[$($key),+],
                action: $action::$act,
            }
        ),*];
        $hvis const $hints: &[$crate::tui::keymap::Hint] = &[$(
            $crate::tui::keymap::Hint { label: $label, desc: $desc, footer: $footer }
        ),*];
    };
}

pub(crate) use keys;

pub fn matches(keys: &[Key], event: KeyEvent) -> bool {
    let mods = event.modifiers.difference(SHIFT);
    keys.iter()
        .any(|&(code, m)| code == event.code && m.difference(SHIFT) == mods)
}

pub fn resolve<A: Copy>(table: &[Binding<A>], event: KeyEvent) -> Option<A> {
    table
        .iter()
        .find(|binding| matches(binding.keys, event))
        .map(|binding| binding.action)
}

pub fn is_text(event: KeyEvent) -> bool {
    if event.modifiers.contains(CTRL) && !event.modifiers.contains(KeyModifiers::ALT) {
        return matches!(event.code, KeyCode::Char('a' | 'e' | 'u' | 'k' | 'w'));
    }
    !event.modifiers.intersects(CTRL | KeyModifiers::ALT)
        && matches!(
            event.code,
            KeyCode::Char(_)
                | KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End
        )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Context {
    Login,
    LoginInput,
    LoginBasic,
    LoginDevice,
    Dashboard,
    DashboardExpanded,
    DashboardRange,
    Browser,
    Sidebar,
    Palette,
    Filter,
    Dialog,
    DialogInfo,
    Created,
    Form,
    FormEdit,
    FormEditText,
    FormEditLines,
    FormViewing,
    FormList,
    FormMap,
    FormExpression,
    Picker,
    Detail,
    Action,
    Tracing,
    TracingForm,
    TracingInput,
    TracingFilters,
    TracingValue,
    Delivery,
    DeliveryForm,
    DeliveryInput,
    TraceDetail,
    Bootstrap,
    BootstrapComplete,
    Tools,
    Log,
    Message,
    Link,
}

impl Context {
    pub fn name(self) -> &'static str {
        match self {
            Context::Login => "Sign in",
            Context::LoginInput => "Sign in (typing)",
            Context::LoginBasic => "Sign in with a password",
            Context::LoginDevice => "Device sign-in",
            Context::Dashboard => "Dashboard",
            Context::DashboardExpanded => "Dashboard panel",
            Context::DashboardRange => "Dashboard range",
            Context::Browser => "Object browser",
            Context::Sidebar => "Sidebar",
            Context::Palette => "Jump palette",
            Context::Filter => "Filter",
            Context::Dialog => "Confirm",
            Context::DialogInfo => "Notice",
            Context::Created => "Result",
            Context::Form => "Edit form",
            Context::FormEdit => "Editing a field",
            Context::FormEditText => "Editing text",
            Context::FormEditLines => "Editing a list",
            Context::FormViewing => "Viewing a value",
            Context::FormList => "List items",
            Context::FormMap => "Map entries",
            Context::FormExpression => "Rule editor",
            Context::Picker => "Picker",
            Context::Detail => "Details",
            Context::Action => "Actions",
            Context::Tracing => "Live tracing",
            Context::TracingForm => "Live tracing",
            Context::TracingInput => "Live tracing (typing)",
            Context::TracingFilters => "Live tracing filters",
            Context::TracingValue => "Live tracing filter value",
            Context::Delivery => "Delivery test",
            Context::DeliveryForm => "Delivery test",
            Context::DeliveryInput => "Delivery test (typing)",
            Context::TraceDetail => "Trace",
            Context::Bootstrap => "Setup",
            Context::BootstrapComplete => "Setup complete",
            Context::Tools => "Tools",
            Context::Log => "HTTP log",
            Context::Message => "Notice",
            Context::Link => "Notice",
        }
    }

    pub fn captures_text(self) -> bool {
        matches!(
            self,
            Context::LoginInput
                | Context::LoginBasic
                | Context::Palette
                | Context::Filter
                | Context::FormEdit
                | Context::FormEditText
                | Context::FormEditLines
                | Context::Picker
                | Context::TracingInput
                | Context::TracingValue
                | Context::DashboardRange
                | Context::DeliveryInput
        )
    }

    pub fn types_question_mark(self) -> bool {
        self.captures_text() && !matches!(self, Context::Palette | Context::Picker)
    }

    pub fn hints(self) -> &'static [Hint] {
        match self {
            Context::Login => login::HINTS,
            Context::LoginInput => login::INPUT_HINTS,
            Context::LoginBasic => login::BASIC_HINTS,
            Context::LoginDevice => login::DEVICE_HINTS,
            Context::Sidebar => sidebar::HINTS,
            Context::Palette => palette::HINTS,
            Context::Log => log_view::HINTS,
            Context::Message => message::HINTS,
            Context::Link => message::LINK_HINTS,
            Context::Dashboard => dashboard::HINTS,
            Context::DashboardExpanded => dashboard::EXPANDED_HINTS,
            Context::DashboardRange => dashboard::RANGE_HINTS,
            Context::Tracing => tracing::SESSION_HINTS,
            Context::TracingForm => tracing::IDLE_HINTS,
            Context::TracingInput => tracing::INPUT_HINTS,
            Context::TracingFilters => tracing::FILTER_HINTS,
            Context::TracingValue => tracing::VALUE_HINTS,
            Context::Delivery => delivery::HINTS,
            Context::DeliveryForm => delivery::IDLE_HINTS,
            Context::DeliveryInput => delivery::INPUT_HINTS,
            Context::TraceDetail => trace_detail::HINTS,
            Context::Browser => browser::HINTS,
            Context::Filter => browser::filters::HINTS,
            Context::Dialog => dialog::HINTS,
            Context::DialogInfo => dialog::INFO_HINTS,
            Context::Created => dialog::COPY_HINTS,
            Context::Form => form::view::HINTS,
            Context::FormEdit => form::view::EDIT_HINTS,
            Context::FormEditText => form::view::TEXT_HINTS,
            Context::FormEditLines => form::view::LIST_HINTS,
            Context::FormViewing => form::view::VIEW_HINTS,
            Context::FormList => form::view::LIST_FRAME_HINTS,
            Context::FormMap => form::view::MAP_FRAME_HINTS,
            Context::FormExpression => form::view::EXPRESSION_FRAME_HINTS,
            Context::Picker => picker::HINTS,
            Context::Detail => detail::HINTS,
            Context::Action => action::HINTS,
            Context::Bootstrap => bootstrap::HINTS,
            Context::BootstrapComplete => bootstrap::COMPLETE_HINTS,
            Context::Tools => tools::HINTS,
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/keymap/tests.rs"]
mod tests;
