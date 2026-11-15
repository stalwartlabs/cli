/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::app::{Cx, ScreenMsg, ViewCx};
use crate::tui::keymap::{Binding, Context, code, key, keys};
use crate::tui::msg::{Effect, HttpUrl};
use crate::tui::widgets::chrome::Crumbs;
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Margin, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Wrap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Back,
    Open,
}

keys! { pub KEYS, pub HINTS: Action;
    "esc" Back "back" true [code(KeyCode::Esc), code(KeyCode::Backspace)],
}

keys! { pub LINK_KEYS, pub LINK_HINTS: Action;
    "o" Open "open the link in a browser" true [key('o')],
    "esc" Back "back" true [code(KeyCode::Esc), code(KeyCode::Backspace)],
}

const TRIAL_URL: &str = "https://license.stalw.art/trial";
const COMPARE_URL: &str = "https://stalw.art/compare#why-isnt-feature-x-open-source";

pub struct State {
    pub title: Box<str>,
    pub text: Box<str>,
    pub url: Option<HttpUrl>,
}

impl State {
    pub fn new(title: impl Into<Box<str>>, text: impl Into<Box<str>>) -> Self {
        State {
            title: title.into(),
            text: text.into(),
            url: None,
        }
    }

    pub fn upsell(feature: &str) -> Self {
        let mut text = String::with_capacity(feature.len() + 320);
        if !feature.is_empty() {
            text.push_str(feature);
            text.push_str(" is part of the Enterprise edition. ");
        }
        text.push_str(
            "Get access to advanced features including \
             multi-tenancy, AI-powered spam filtering, alerts, and more.\n\n\
             Start a 30-day free trial: ",
        );
        text.push_str(TRIAL_URL);
        text.push_str("\nWhy is this not free? ");
        text.push_str(COMPARE_URL);
        State {
            title: "Unlock Enterprise Features".into(),
            text: text.into(),
            url: HttpUrl::parse(TRIAL_URL),
        }
    }

    pub fn unknown(view: &str) -> Self {
        let mut text = String::with_capacity(view.len() + 64);
        text.push_str("This console cannot show ");
        text.push_str(view);
        text.push_str(" yet. Use the palette (:) or the sidebar to go elsewhere.");
        State::new("Unknown view", text)
    }
}

pub fn context(state: &State) -> Context {
    match state.url {
        Some(_) => Context::Link,
        None => Context::Message,
    }
}

pub fn keys(state: &State) -> &'static [Binding<Action>] {
    match state.url {
        Some(_) => LINK_KEYS,
        None => KEYS,
    }
}

pub fn update(state: &mut State, cx: &mut Cx<'_>, msg: ScreenMsg<'_, Action>) {
    match msg {
        ScreenMsg::Key(Action::Back) => cx.back(),
        ScreenMsg::Key(Action::Open) => {
            if let Some(url) = &state.url {
                cx.fx.push(Effect::OpenUrl(url.clone()));
            }
        }
        _ => {}
    }
}

pub fn render(state: &mut State, frame: &mut Frame, area: Rect, cx: &ViewCx<'_>) {
    let theme = cx.theme;
    let block = theme.panel(Line::from(state.title.as_ref()), cx.focused);
    let inner = block.inner(area).inner(Margin::new(2, 1));
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Text::styled(state.text.as_ref(), theme.muted())).wrap(Wrap { trim: true }),
        inner,
    );
}

pub fn crumbs(state: &State, out: &mut Crumbs) {
    out.push(&state.title);
}
