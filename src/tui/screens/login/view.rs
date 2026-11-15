/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use super::{DeviceView, Field, Stage, State, Step, context};
use crate::auth::login::BasicReason;
use crate::tui::session::host;
use crate::tui::theme::{Palette, Theme};
use crate::tui::widgets::chrome::{FlashSlot, FooterInfo, footer, pad};
use crate::tui::widgets::input::Input;
use crate::tui::widgets::spinner;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{LineGauge, Paragraph, Wrap};
use std::fmt::Write;
use std::time::Instant;
use tui_big_text::{BigText, PixelSize};

const LARGE_LOGO: &str = include_str!("../../../cli/logo/large.ans");
const COMPACT_LOGO: &str = include_str!("../../../cli/logo/compact.ans");
const LARGE_LOGO_WIDTH: u16 = 91;
const COMPACT_LOGO_WIDTH: u16 = 79;
const CARD_WIDTH: u16 = 92;
const CARD_HEIGHT: u16 = 16;
const LABEL_WIDTH: usize = 10;
const INPUT_WIDTH: u16 = 52;
const CLI_HINT: &str = "Run stalwart-cli --help for the command-line interface.";
const PHISHING: &str =
    "Only approve a code shown by this terminal. Never approve a code someone sent you.";
const SECONDS_PER_MINUTE: u64 = 60;

fn logo_lines(ans: &str, theme: &Theme) -> Vec<Line<'static>> {
    ans.lines()
        .map(|line| {
            let mut spans = Vec::new();
            let mut style = Style::new();
            for (i, chunk) in line.split('\x1b').enumerate() {
                let text = if i == 0 {
                    chunk
                } else {
                    let Some((code, rest)) =
                        chunk.strip_prefix('[').and_then(|c| c.split_once('m'))
                    else {
                        continue;
                    };
                    style = if code == "0" {
                        Style::new()
                    } else if code == "1" {
                        style.add_modifier(Modifier::BOLD).fg(theme.fg)
                    } else if code.starts_with("38;2;") {
                        style.fg(theme.brand)
                    } else {
                        style
                    };
                    rest
                };
                if !text.is_empty() {
                    spans.push(Span::styled(text.to_string(), style));
                }
            }
            Line::from(spans)
        })
        .collect()
}

pub struct Logo {
    palette: Palette,
    large: bool,
    height: u16,
    paragraph: Paragraph<'static>,
}

pub fn render(state: &mut State, frame: &mut Frame, theme: &Theme, flash: &FlashSlot, tick: u64) {
    let area = frame.area();
    let [body, foot] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let large = body.width >= LARGE_LOGO_WIDTH + 4;
    let stale = state
        .logo
        .as_ref()
        .is_none_or(|logo| logo.palette != theme.palette || logo.large != large);
    if stale {
        let ans = if large { LARGE_LOGO } else { COMPACT_LOGO };
        let lines = logo_lines(ans, theme);
        state.logo = Some(Logo {
            palette: theme.palette,
            large,
            height: u16::try_from(lines.len()).unwrap_or(0),
            paragraph: Paragraph::new(lines),
        });
    }
    let logo_height = state.logo.as_ref().map_or(0, |logo| logo.height);
    let logo_width = if large {
        LARGE_LOGO_WIDTH
    } else {
        COMPACT_LOGO_WIDTH
    };
    let column = body.centered(
        Constraint::Length(CARD_WIDTH.min(body.width)),
        Constraint::Length(logo_height + 2 + CARD_HEIGHT),
    );
    let [logo_area, tagline, _, card] = Layout::vertical([
        Constraint::Length(logo_height),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(CARD_HEIGHT),
    ])
    .areas(column);
    if let Some(logo) = &state.logo {
        frame.render_widget(
            &logo.paragraph,
            logo_area.centered_horizontally(Constraint::Length(logo_width)),
        );
    }
    frame.render_widget(
        Paragraph::new(Line::styled("management console", theme.muted()).right_aligned()),
        tagline,
    );
    let title = match state.stage {
        Stage::Input => Line::from(" Sign in "),
        _ => Line::from(vec![
            Span::raw(" Sign in to "),
            Span::raw(host(state.url.value())),
            Span::raw(" "),
        ]),
    };
    let block = theme.panel(title, true);
    let inner = block.inner(card).inner(Margin::new(2, 1));
    frame.render_widget(block, card);
    let now = Instant::now();
    match &state.stage {
        Stage::Device(view) => render_device(state, view, frame, inner, theme, tick, now),
        _ => {
            let lines = form_lines(state, theme, tick);
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
    }
    let info = FooterInfo {
        ctx: context(state),
        available: &|_| true,
        sidebar_hidden: false,
    };
    footer(frame, foot, theme, &info, flash.get());
}

fn form_lines<'a>(state: &'a mut State, theme: &Theme, tick: u64) -> Vec<Line<'a>> {
    let mut lines = Vec::with_capacity(16);
    if let Some(notice) = &state.notice {
        lines.push(mark_line(theme, "  ! ", theme.warn, notice));
        lines.push(Line::raw(""));
    }
    if let (Stage::Basic, Some(reason)) = (&state.stage, &state.reason) {
        lines.push(mark_line(theme, "  ! ", theme.warn, &reason_text(reason)));
        lines.push(Line::raw(""));
    }
    let editing = matches!(state.stage, Stage::Input | Stage::Basic);
    let field = state.field;
    let basic = matches!(state.stage, Stage::Basic);
    let spin = spinner::frame(tick);
    let status: Option<&'static str> = match &state.stage {
        Stage::Discovering { step, .. } => Some(match step {
            Step::Stored => " Looking for a saved session…",
            Step::Discover => " Discovering the sign-in method…",
            Step::DeviceCode => " Requesting a device code…",
        }),
        Stage::Connecting => Some(" Connecting: session, schema, account…"),
        _ => None,
    };
    let error = state.error.clone();
    let api_key = state.api_key.is_some();
    lines.push(field_line(
        theme,
        "Server",
        &mut state.url,
        editing && !basic && field == Field::Url,
    ));
    lines.push(Line::raw(""));
    if api_key {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("Account   ", theme.muted()),
            Span::styled("API key from the command line", theme.text()),
        ]));
    } else {
        lines.push(field_line(
            theme,
            "Account",
            &mut state.account,
            editing && field == Field::Account,
        ));
        if basic {
            lines.push(Line::raw(""));
            lines.push(field_line(
                theme,
                "Password",
                &mut state.password,
                field == Field::Password,
            ));
        }
    }
    lines.push(Line::raw(""));
    match (status, error) {
        (Some(text), _) => lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(spin, Style::new().fg(theme.warn)),
            Span::styled(text, theme.text()),
        ])),
        (None, Some(error)) => lines.push(Line::from(vec![
            Span::styled(
                "  ✗ ",
                Style::new().fg(theme.err).add_modifier(Modifier::BOLD),
            ),
            Span::styled(String::from(error), Style::new().fg(theme.err)),
        ])),
        (None, None) if basic => {
            lines.push(Line::styled(
                "  Accounts with two-factor authentication need an app password or an API key.",
                theme.muted(),
            ));
            lines.push(Line::styled(
                "  Sent as HTTP Basic on every request; nothing is written to disk.",
                theme.muted(),
            ));
        }
        (None, None) => lines.push(Line::styled(
            "  The sign-in method is discovered from the server and the account's domain.",
            theme.muted(),
        )),
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(CLI_HINT, theme.muted()));
    lines
}

fn reason_text(reason: &BasicReason) -> String {
    match reason {
        BasicReason::Preferred => "Signing in with a password.".into(),
        BasicReason::Recovery => {
            "The server is in recovery mode; sign in with the recovery administrator password."
                .into()
        }
        BasicReason::NoDeviceSupport => {
            "Device sign-in is not available on this server; sign in with a password.".into()
        }
        BasicReason::NoClientId => {
            "No OAuth client is available; pass --client-id or sign in with a password.".into()
        }
        BasicReason::DcrRefused(error) => {
            let mut text = String::from("The server refused to register this console: ");
            text.push_str(error);
            text
        }
        BasicReason::Discovery(error) => {
            let mut text = String::from("Sign-in discovery failed: ");
            text.push_str(error);
            text
        }
        BasicReason::External { issuer, reason } => {
            let mut text = reason_text(reason);
            text.push_str(" This account is managed by the identity provider ");
            text.push_str(issuer);
            text.push_str(
                ", so its password may not work here: use an app password or an API key, \
                 or pass --client-id with a client registered there for device sign-in.",
            );
            text
        }
    }
}

fn mark_line<'a>(
    theme: &Theme,
    mark: &'static str,
    color: ratatui::style::Color,
    text: &str,
) -> Line<'a> {
    Line::from(vec![
        Span::styled(mark, Style::new().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(text.to_string(), theme.text()),
    ])
}

fn render_device(
    state: &State,
    view: &DeviceView,
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    tick: u64,
    now: Instant,
) {
    let grant = &view.grant;
    let [top, code, rest] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(4),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("  Server    ", theme.muted()),
                Span::styled(state.url.value().to_string(), theme.text()),
            ]),
            Line::from(vec![
                Span::styled("  Open ", theme.text()),
                Span::styled(
                    grant.verification_uri.clone(),
                    Style::new()
                        .fg(theme.accent)
                        .add_modifier(Modifier::UNDERLINED),
                ),
                Span::styled(" and enter this code:", theme.text()),
            ]),
        ]),
        top,
    );
    frame.render_widget(
        BigText::builder()
            .pixel_size(PixelSize::Quadrant)
            .style(Style::new().fg(theme.brand).add_modifier(Modifier::BOLD))
            .lines(vec![grant.user_code.as_str().into()])
            .centered()
            .build(),
        code,
    );
    let elapsed = now.saturating_duration_since(view.issued);
    let remaining = view.lifetime.saturating_sub(elapsed).as_secs();
    let ratio = if view.lifetime.is_zero() {
        0.0
    } else {
        (remaining as f64 / view.lifetime.as_secs_f64()).clamp(0.0, 1.0)
    };
    let mut lines = vec![Line::styled(grant.user_code.clone(), theme.title()).centered()];
    if let Some(complete) = &grant.verification_uri_complete {
        lines.push(Line::from(vec![
            Span::styled("  or open ", theme.muted()),
            Span::styled(complete.clone(), Style::new().fg(theme.accent)),
            Span::styled("  (code pre-filled)", theme.muted()),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("  ", theme.text()),
        Span::styled(spinner::frame(tick), Style::new().fg(theme.warn)),
        Span::styled(" Waiting for approval", theme.text()),
    ]));
    let [info, gauge, warn] = Layout::vertical([
        Constraint::Length(u16::try_from(lines.len()).unwrap_or(3)),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(rest);
    frame.render_widget(Paragraph::new(lines), info);
    let color = if ratio > 0.25 { theme.ok } else { theme.warn };
    let mut countdown = String::with_capacity(8);
    let _ = write!(
        countdown,
        "{:02}:{:02}  ",
        remaining / SECONDS_PER_MINUTE,
        remaining % SECONDS_PER_MINUTE
    );
    frame.render_widget(
        LineGauge::default()
            .ratio(ratio)
            .label(Line::from(vec![
                Span::styled("  Code expires in ", theme.muted()),
                Span::styled(countdown, theme.title()),
            ]))
            .filled_symbol("━")
            .unfilled_symbol("─")
            .filled_style(Style::new().fg(color))
            .unfilled_style(Style::new().fg(theme.border)),
        gauge,
    );
    frame.render_widget(
        Paragraph::new(mark_line(theme, "  ! ", theme.warn, PHISHING)).wrap(Wrap { trim: false }),
        warn,
    );
}

fn field_line<'a>(
    theme: &Theme,
    label: &'static str,
    input: &'a mut Input,
    active: bool,
) -> Line<'a> {
    let label_style = if active {
        theme.strong()
    } else {
        theme.muted()
    };
    let mut line = Line::from(vec![
        Span::styled(
            if active { "▌ " } else { "  " },
            Style::new().fg(theme.brand),
        ),
        Span::styled(label, label_style),
        Span::raw(pad(LABEL_WIDTH.saturating_sub(label.len()))),
    ]);
    line.spans
        .extend(input.line(theme, INPUT_WIDTH, active).spans);
    line
}
