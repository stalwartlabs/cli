/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::jmap::live::TraceFrame;
use crate::jmap::sse::StreamClient;
use crate::tui::exec::Cancel;
use crate::tui::msg::{Msg, StreamEvent, StreamId, StreamSpec};
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

const MAX_ATTEMPTS: u8 = 5;
const STREAM_PANICKED: &str = "the live stream stopped after an internal error";
const BASE_DELAY: Duration = Duration::from_secs(2);
const MAX_DELAY: Duration = Duration::from_secs(10);
const STABLE_AFTER: Duration = Duration::from_secs(30);
const SLEEP_STEP: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub struct Backoff {
    max_attempts: u8,
    attempt: u8,
    ever_open: bool,
    open_since: Option<Instant>,
}

impl Backoff {
    pub fn new(max_attempts: u8) -> Self {
        Backoff {
            max_attempts,
            attempt: 0,
            ever_open: false,
            open_since: None,
        }
    }

    pub fn opened(&mut self, now: Instant) {
        self.ever_open = true;
        self.open_since = Some(now);
    }

    pub fn received(&mut self) {
        self.attempt = 0;
    }

    pub fn failed(&mut self, error: &Error, now: Instant) -> Option<(u8, Duration)> {
        let open_since = self.open_since.take();
        if !self.ever_open || !retryable(error) {
            return None;
        }
        if open_since.is_some_and(|since| now.saturating_duration_since(since) >= STABLE_AFTER) {
            self.attempt = 0;
        }
        self.attempt = self.attempt.saturating_add(1);
        (self.attempt <= self.max_attempts).then(|| (self.attempt, delay(self.attempt)))
    }
}

fn delay(attempt: u8) -> Duration {
    let doublings = u32::from(attempt.saturating_sub(1)).min(8);
    BASE_DELAY.saturating_mul(1 << doublings).min(MAX_DELAY)
}

fn retryable(error: &Error) -> bool {
    match error {
        Error::Network(_) | Error::Io(_) | Error::RateLimited | Error::UnexpectedResponse(_) => {
            true
        }
        Error::HttpStatus(status) => *status >= 500 || *status == 408,
        _ => false,
    }
}

enum Frame {
    Event(StreamEvent),
    Ping,
    End,
}

enum Outcome {
    Stop,
    Ended,
    Failed(Error),
}

struct Pump<'a> {
    id: StreamId,
    tx: &'a Sender<Msg>,
    cancel: &'a Cancel,
    backoff: Backoff,
}

impl Pump<'_> {
    fn send(&self, event: StreamEvent) -> bool {
        !self.cancel.is_cancelled() && self.tx.send(Msg::Stream(self.id, event)).is_ok()
    }

    fn run(&mut self, mut next: impl FnMut() -> Result<Frame>) -> Outcome {
        self.backoff.opened(Instant::now());
        if !self.send(StreamEvent::Open) {
            return Outcome::Stop;
        }
        loop {
            let frame = next();
            if self.cancel.is_cancelled() {
                return Outcome::Stop;
            }
            match frame {
                Ok(Frame::Event(event)) => {
                    self.backoff.received();
                    if !self.send(event) {
                        return Outcome::Stop;
                    }
                }
                Ok(Frame::Ping) => {}
                Ok(Frame::End) => return Outcome::Ended,
                Err(error) => return Outcome::Failed(error),
            }
        }
    }

    fn session(&mut self, spec: &StreamSpec, client: &StreamClient) -> Outcome {
        let opened = match spec {
            StreamSpec::Metrics { ids, interval_s } => {
                let names = ids.iter().map(|id| &**id).collect::<Vec<_>>();
                client.live_metrics(*interval_s, &names).map(|mut stream| {
                    self.run(|| {
                        Ok(match stream.next_frame()? {
                            Some(samples) => Frame::Event(StreamEvent::Metrics(samples)),
                            None => Frame::End,
                        })
                    })
                })
            }
            StreamSpec::Tracing { filter, pairs } => {
                let pairs = pairs
                    .iter()
                    .map(|(key, value)| (&**key, &**value))
                    .collect::<Vec<_>>();
                client
                    .live_tracing(filter.as_deref(), &pairs)
                    .map(|mut stream| {
                        self.run(|| {
                            Ok(match stream.next_frame()? {
                                Some(TraceFrame::Events(events)) => {
                                    Frame::Event(StreamEvent::Trace(events))
                                }
                                Some(TraceFrame::Ping) => Frame::Ping,
                                None => Frame::End,
                            })
                        })
                    })
            }
            StreamSpec::Delivery { target } => client.live_delivery(target).map(|mut stream| {
                self.run(|| {
                    Ok(match stream.next_stage()? {
                        Some(stage) => Frame::Event(StreamEvent::Stage(Box::new(stage))),
                        None => Frame::End,
                    })
                })
            }),
        };
        opened.unwrap_or_else(Outcome::Failed)
    }
}

impl StreamSpec {
    fn max_attempts(&self) -> u8 {
        match self {
            StreamSpec::Delivery { .. } => 0,
            StreamSpec::Metrics { .. } | StreamSpec::Tracing { .. } => MAX_ATTEMPTS,
        }
    }

    fn ends_by_itself(&self) -> bool {
        matches!(self, StreamSpec::Delivery { .. })
    }
}

pub fn spawn(
    id: StreamId,
    spec: StreamSpec,
    client: StreamClient,
    tx: Sender<Msg>,
) -> io::Result<Cancel> {
    let cancel = Cancel::default();
    let handle = cancel.clone();
    thread::Builder::new()
        .name("tui-stream".into())
        .spawn(move || supervise(id, &tx, || run(id, &spec, &client, &tx, &handle)))?;
    Ok(cancel)
}

fn supervise(id: StreamId, tx: &Sender<Msg>, run: impl FnOnce()) {
    if panic::catch_unwind(AssertUnwindSafe(run)).is_err() {
        let error = Error::UnexpectedResponse(STREAM_PANICKED.into());
        let _ = tx.send(Msg::Stream(id, StreamEvent::Closed(Some(Box::new(error)))));
    }
}

fn run(id: StreamId, spec: &StreamSpec, client: &StreamClient, tx: &Sender<Msg>, cancel: &Cancel) {
    let mut pump = Pump {
        id,
        tx,
        cancel,
        backoff: Backoff::new(spec.max_attempts()),
    };
    loop {
        let error = match pump.session(spec, client) {
            Outcome::Stop => return,
            Outcome::Ended if spec.ends_by_itself() => {
                pump.send(StreamEvent::Closed(None));
                return;
            }
            Outcome::Ended => Error::Io(io::ErrorKind::UnexpectedEof.into()),
            Outcome::Failed(error) => error,
        };
        if cancel.is_cancelled() {
            return;
        }
        match pump.backoff.failed(&error, Instant::now()) {
            Some((attempt, delay)) => {
                if !pump.send(StreamEvent::Reconnecting { attempt }) || !sleep(delay, cancel) {
                    return;
                }
            }
            None => {
                pump.send(StreamEvent::Closed(Some(Box::new(error))));
                return;
            }
        }
    }
}

fn sleep(total: Duration, cancel: &Cancel) -> bool {
    let start = Instant::now();
    while !cancel.is_cancelled() {
        let elapsed = start.elapsed();
        if elapsed >= total {
            return true;
        }
        thread::sleep(SLEEP_STEP.min(total - elapsed));
    }
    false
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/runtime/stream/tests.rs"]
mod tests;
