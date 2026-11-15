/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Error;
use crate::jmap::live::{DeliveryStage, LiveSample, TraceEvent};
use crate::plan::bulk::BulkEvent;
use crate::tui::exec::{JobCx, Slot, SlotTable, Ticket};
use crate::tui::job::auth::{AuthJob, AuthOutput};
use crate::tui::job::metrics::{MetricsJob, MetricsOutput};
use crate::tui::job::objects::{ObjectsJob, ObjectsOutput};
use crate::tui::job::session::{SessionJob, SessionOutput};
use ratatui::crossterm::event::Event;
use std::io;
use std::path::PathBuf;
use std::time::Instant;

pub enum Msg {
    Input(Event),
    Done(Ticket, JobOutput),
    JobPanicked(Ticket),
    Stream(StreamId, StreamEvent),
    Tick(Instant),
    Resumed(Ticket, Option<Box<io::Error>>),
    InputFailed(Box<io::Error>),
    Progress(Ticket, JobProgress),
}

#[derive(Debug)]
pub enum JobProgress {
    Log(Box<str>),
    Bulk(BulkEvent),
}

const _: () = assert!(size_of::<Msg>() <= 64);

pub enum Job {
    Session(SessionJob),
    Auth(AuthJob),
    Metrics(MetricsJob),
    Objects(ObjectsJob),
}

pub enum JobOutput {
    Session(SessionOutput),
    Auth(AuthOutput),
    Metrics(MetricsOutput),
    Objects(ObjectsOutput),
}

impl JobOutput {
    pub fn error(&self) -> Option<&Error> {
        match self {
            JobOutput::Session(output) => output.error(),
            JobOutput::Auth(output) => output.error(),
            JobOutput::Metrics(output) => output.error(),
            JobOutput::Objects(output) => output.error(),
        }
    }

    pub fn session_expired(&self) -> bool {
        matches!(
            self.error(),
            Some(Error::SessionExpired | Error::AuthFailed)
        )
    }
}

impl Job {
    pub fn is_read_only(&self) -> bool {
        match self {
            Job::Metrics(_) => true,
            Job::Objects(job) => job.is_read_only(),
            Job::Session(_) | Job::Auth(_) => false,
        }
    }

    pub fn run(self, cx: &JobCx) -> JobOutput {
        match self {
            Job::Session(job) => JobOutput::Session(job.run(cx)),
            Job::Auth(job) => JobOutput::Auth(job.run(cx)),
            Job::Metrics(job) => JobOutput::Metrics(job.run(cx)),
            Job::Objects(job) => JobOutput::Objects(job.run(cx)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StreamId(u32);

impl StreamId {
    pub fn start(slots: &mut SlotTable) -> Self {
        StreamId(slots.start(Slot::Stream).generation)
    }

    pub fn ticket(self) -> Ticket {
        Ticket {
            slot: Slot::Stream,
            generation: self.0,
        }
    }
}

pub enum StreamSpec {
    Metrics {
        ids: Box<[Box<str>]>,
        interval_s: u32,
    },
    Tracing {
        filter: Option<Box<str>>,
        pairs: Box<[(Box<str>, Box<str>)]>,
    },
    Delivery {
        target: Box<str>,
    },
}

pub enum StreamEvent {
    Open,
    Reconnecting { attempt: u8 },
    Metrics(Vec<LiveSample>),
    Trace(Vec<TraceEvent>),
    Stage(Box<DeliveryStage>),
    Closed(Option<Box<Error>>),
}

impl StreamEvent {
    pub fn session_expired(&self) -> bool {
        matches!(
            self,
            StreamEvent::Closed(Some(error)) if matches!(**error, Error::SessionExpired | Error::AuthFailed)
        )
    }
}

pub enum External {
    Editor { path: PathBuf, ticket: Ticket },
}

pub enum Effect {
    Spawn(Ticket, Job),
    OpenStream(StreamId, StreamSpec),
    CloseStream(StreamId),
    Suspend(External),
    OpenUrl(HttpUrl),
    Copy(Box<str>),
    SavePrefs,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpUrl(Box<str>);

impl HttpUrl {
    pub fn parse(url: &str) -> Option<Self> {
        let url = url.trim();
        let (scheme, rest) = url.split_once("://")?;
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        (hashify::set_ignore_case!(scheme.as_bytes(), "http", "https")
            && !host.is_empty()
            && !url.chars().any(|c| c.is_whitespace() || c.is_control()))
        .then(|| HttpUrl(url.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Default)]
pub struct Effects(Vec<Effect>);

impl Effects {
    pub fn push(&mut self, effect: Effect) {
        self.0.push(effect);
    }

    pub fn spawn(&mut self, ticket: Ticket, job: Job) {
        self.0.push(Effect::Spawn(ticket, job));
    }

    pub fn drain(&mut self) -> impl Iterator<Item = Effect> + '_ {
        self.0.drain(..)
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/tui/msg/tests.rs"]
mod tests;
