/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{event, queue};
use std::io::{self, Stdout, Write};
use std::panic;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub struct TerminalGuard {
    terminal: Term,
    active: Arc<AtomicBool>,
}

pub fn write_enter(out: &mut impl Write) -> io::Result<()> {
    queue!(out, EnterAlternateScreen, EnableBracketedPaste, Hide)?;
    out.flush()
}

pub fn write_restore(out: &mut impl Write) -> io::Result<()> {
    queue!(out, DisableBracketedPaste, LeaveAlternateScreen, Show)?;
    out.flush()
}

fn restore_terminal(active: &AtomicBool) -> io::Result<()> {
    if active.swap(false, Ordering::SeqCst) {
        let restored = write_restore(&mut io::stdout());
        disable_raw_mode()?;
        restored?;
    }
    Ok(())
}

fn on_panic(active: &AtomicBool, ui_thread: bool, report: impl FnOnce()) {
    if ui_thread {
        let _ = restore_terminal(active);
        report();
    } else if !active.load(Ordering::SeqCst) {
        report();
    }
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        let active = Arc::new(AtomicBool::new(false));
        let hook_flag = Arc::clone(&active);
        let ui = thread::current().id();
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            on_panic(&hook_flag, thread::current().id() == ui, || previous(info));
        }));
        let mut guard = TerminalGuard {
            terminal: Terminal::new(CrosstermBackend::new(io::stdout()))?,
            active,
        };
        enable_raw_mode()?;
        guard.active.store(true, Ordering::SeqCst);
        write_enter(guard.terminal.backend_mut())?;
        guard.terminal.clear()?;
        Ok(guard)
    }

    pub fn terminal(&mut self) -> &mut Term {
        &mut self.terminal
    }

    pub fn suspend(&mut self) -> io::Result<()> {
        restore_terminal(&self.active)
    }

    pub fn resume(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        self.active.store(true, Ordering::SeqCst);
        write_enter(self.terminal.backend_mut())?;
        while event::poll(Duration::ZERO)? {
            event::read()?;
        }
        self.terminal.clear()
    }

    pub fn restore(self) -> io::Result<()> {
        restore_terminal(&self.active)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = restore_terminal(&self.active);
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/tui/term/tests.rs"]
mod tests;
