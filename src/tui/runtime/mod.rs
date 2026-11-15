/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::jmap::sse::StreamClient;
use crate::tui::app::App;
use crate::tui::exec::{Cancel, Executor};
use crate::tui::msg::{Effect, Effects, External, HttpUrl, Msg, StreamEvent, StreamId, StreamSpec};
use crate::tui::term::TerminalGuard;
use crate::tui::theme::Level;
use encodify::base64::STANDARD;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

mod stream;

const INPUT_POLL: Duration = Duration::from_millis(200);
const PARK_POLL: Duration = Duration::from_millis(20);
const PARK_WAIT: Duration = Duration::from_millis(500);
const RUNNING: u8 = 0;
const SUSPENDING: u8 = 1;
const PARKED: u8 = 2;
const STOP: u8 = 3;
const INPUT_PANICKED: &str = "the input thread stopped after an internal error";

#[derive(Clone, Default)]
pub struct InputGate(Arc<AtomicU8>);

impl InputGate {
    fn state(&self) -> u8 {
        self.0.load(Ordering::SeqCst)
    }

    fn set(&self, state: u8) {
        self.0.store(state, Ordering::SeqCst);
    }

    pub fn suspend(&self) {
        self.set(SUSPENDING);
        let start = Instant::now();
        while self.state() == SUSPENDING && start.elapsed() < PARK_WAIT {
            thread::sleep(PARK_POLL);
        }
    }

    pub fn resume(&self) {
        self.set(RUNNING);
    }

    pub fn stop(&self) {
        self.set(STOP);
    }
}

fn supervise_input(tx: &Sender<Msg>, run: impl FnOnce()) {
    if panic::catch_unwind(AssertUnwindSafe(run)).is_err() {
        let _ = tx.send(Msg::InputFailed(Box::new(io::Error::other(INPUT_PANICKED))));
    }
}

fn input_loop(tx: &Sender<Msg>, gate: &InputGate) {
    loop {
        match gate.state() {
            STOP => return,
            SUSPENDING => {
                gate.0
                    .compare_exchange(SUSPENDING, PARKED, Ordering::SeqCst, Ordering::SeqCst)
                    .ok();
                continue;
            }
            PARKED => {
                thread::sleep(PARK_POLL);
                continue;
            }
            _ => {}
        }
        let event = match event::poll(INPUT_POLL) {
            Ok(false) => continue,
            Ok(true) => event::read(),
            Err(err) => Err(err),
        };
        let msg = match event {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Release => continue,
            Ok(event @ (Event::Key(_) | Event::Resize(..) | Event::Paste(_))) => Msg::Input(event),
            Ok(_) => continue,
            Err(err) => {
                let _ = tx.send(Msg::InputFailed(Box::new(err)));
                return;
            }
        };
        if tx.send(msg).is_err() {
            return;
        }
    }
}

pub fn run(mut app: App, mut guard: TerminalGuard) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let gate = InputGate::default();
    {
        let tx = tx.clone();
        let gate = gate.clone();
        thread::Builder::new()
            .name("tui-input".into())
            .spawn(move || supervise_input(&tx, || input_loop(&tx, &gate)))?;
    }
    let mut runtime = Runtime {
        exec: Executor::new(tx.clone()),
        tx,
        gate,
        streams: Vec::new(),
    };
    let mut fx = Effects::default();
    app.start(&mut fx, Instant::now());
    runtime.execute(&mut app, &mut fx, &mut guard)?;
    let result = runtime.event_loop(&mut app, &rx, &mut fx, &mut guard);
    runtime.gate.stop();
    runtime.close_streams(|_| true);
    let restored = guard.restore();
    result?;
    Ok(restored?)
}

struct Runtime {
    exec: Executor,
    tx: Sender<Msg>,
    gate: InputGate,
    streams: Vec<(StreamId, Cancel)>,
}

impl Runtime {
    fn event_loop(
        &mut self,
        app: &mut App,
        rx: &mpsc::Receiver<Msg>,
        fx: &mut Effects,
        guard: &mut TerminalGuard,
    ) -> Result<()> {
        loop {
            if app.dirty {
                let now = Instant::now();
                guard.terminal().draw(|frame| app.render(frame, now))?;
                app.dirty = false;
            }
            let now = Instant::now();
            let first = match app.next_deadline(now) {
                None => match rx.recv() {
                    Ok(msg) => msg,
                    Err(_) => return Ok(()),
                },
                Some(deadline) => match rx.recv_timeout(deadline.saturating_duration_since(now)) {
                    Ok(msg) => msg,
                    Err(RecvTimeoutError::Timeout) => Msg::Tick(Instant::now()),
                    Err(RecvTimeoutError::Disconnected) => return Ok(()),
                },
            };
            if let Msg::InputFailed(err) = first {
                return Err((*err).into());
            }
            self.dispatch(app, first, fx, guard)?;
            while let Ok(msg) = rx.try_recv() {
                if let Msg::InputFailed(err) = msg {
                    return Err((*err).into());
                }
                self.dispatch(app, msg, fx, guard)?;
            }
            if app.quit {
                return Ok(());
            }
        }
    }

    fn dispatch(
        &mut self,
        app: &mut App,
        msg: Msg,
        fx: &mut Effects,
        guard: &mut TerminalGuard,
    ) -> Result<()> {
        match &msg {
            Msg::Done(..) | Msg::JobPanicked(_) => self.exec.finished(&app.slots)?,
            Msg::Stream(id, StreamEvent::Closed(_)) => self.close_streams(|open| open == *id),
            _ => {}
        }
        app.update(msg, fx, Instant::now());
        self.execute(app, fx, guard)
    }

    fn execute(
        &mut self,
        app: &mut App,
        fx: &mut Effects,
        guard: &mut TerminalGuard,
    ) -> Result<()> {
        for effect in fx.drain() {
            match effect {
                Effect::Spawn(ticket, job) => self.exec.submit(ticket, job)?,
                Effect::OpenStream(id, spec) => self.open_stream(app, id, spec)?,
                Effect::CloseStream(id) => self.close_streams(|open| open == id),
                Effect::Suspend(External::Editor { path, ticket }) => {
                    self.gate.suspend();
                    guard.suspend()?;
                    let outcome = run_editor(&path);
                    guard.resume()?;
                    self.gate.resume();
                    app.dirty = true;
                    let _ = self
                        .tx
                        .send(Msg::Resumed(ticket, outcome.err().map(Box::new)));
                }
                Effect::OpenUrl(url) => {
                    if let Err(error) = open_url(&url) {
                        app.open_failed(&url, &error, Instant::now());
                    }
                }
                Effect::Copy(text) => {
                    let mut out = io::stdout().lock();
                    write_osc52(&mut out, &text)?;
                }
                Effect::SavePrefs => {
                    if let Some(path) = &app.launch.prefs_path
                        && let Err(error) = app.prefs.save(path, app.launch.writer_id)
                    {
                        let mut text = String::from("could not save preferences: ");
                        text.push_str(&error.to_string());
                        app.flash.set(Level::Warn, text, Instant::now());
                        app.dirty = true;
                    }
                }
                Effect::Quit => app.quit = true,
            }
        }
        self.close_streams(|id| !app.slots.is_current(&id.ticket()));
        Ok(())
    }

    fn open_stream(&mut self, app: &App, id: StreamId, spec: StreamSpec) -> io::Result<()> {
        let Some(conn) = &app.conn else {
            return Ok(());
        };
        match StreamClient::new(&conn.url, conn.credential.clone(), app.launch.tls) {
            Ok(client) => {
                let cancel = stream::spawn(id, spec, client, self.tx.clone())?;
                self.streams.push((id, cancel));
            }
            Err(error) => {
                let _ = self
                    .tx
                    .send(Msg::Stream(id, StreamEvent::Closed(Some(Box::new(error)))));
            }
        }
        Ok(())
    }

    fn close_streams(&mut self, mut pred: impl FnMut(StreamId) -> bool) {
        self.streams.retain(|(id, cancel)| {
            let close = pred(*id);
            if close {
                cancel.cancel();
            }
            !close
        });
    }
}

pub fn write_osc52(out: &mut impl Write, text: &str) -> io::Result<()> {
    let mut seq = String::with_capacity(STANDARD.encoded_len(text.len()) + 8);
    seq.push_str("\x1b]52;c;");
    STANDARD.encode_append(text, &mut seq);
    seq.push('\x07');
    out.write_all(seq.as_bytes())?;
    out.flush()
}

fn run_editor(path: &std::path::Path) -> io::Result<()> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".into());
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("vi");
    let status = Command::new(program).args(parts).arg(path).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{program} exited with {status}")))
    }
}

fn open_url(url: &HttpUrl) -> io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        Command::new("xdg-open")
    };
    let mut child = command
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    thread::Builder::new()
        .name("tui-open".into())
        .spawn(move || child.wait())
        .map(drop)
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/runtime/tests.rs"]
mod tests;
