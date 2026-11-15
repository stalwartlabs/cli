/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::keymap::{code, ctrl, key, keys, upper};
use ratatui::crossterm::event::KeyCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalAction {
    Palette,
    Help,
    ToggleSidebar,
    FocusSidebar,
    ToggleTheme,
    Reload,
    HttpLog,
    Accounts,
    Logout,
    Back,
    Quit,
}

keys! { pub KEYS, pub HINTS: GlobalAction;
    ":" Palette "jump to any view" true [key(':')],
    "? F1" Help "help" true [key('?'), code(KeyCode::F(1))],
    "s" ToggleSidebar "sidebar: always / auto-hide" false [key('s')],
    "←" FocusSidebar "focus the sidebar" false [code(KeyCode::Left)],
    "^t" ToggleTheme "theme: dark / light / 16 colours / mono" false [ctrl('t')],
    "R" Reload "reload view" false [upper('R')],
    "@" HttpLog "HTTP log (--debug)" false [key('@')],
    "A" Accounts "switch account" false [upper('A')],
    "L" Logout "sign out" false [upper('L')],
    "esc" Back "back" false [code(KeyCode::Esc), code(KeyCode::Backspace)],
    "q ^c" Quit "quit" false [key('q'), ctrl('c')],
}
