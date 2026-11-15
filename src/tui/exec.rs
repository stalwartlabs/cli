/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::job::objects::ObjectsSlot;
use crate::tui::msg::{Job, JobProgress, Msg};
use ahash::AHashMap;
use std::collections::VecDeque;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;

pub const MAX_JOBS: usize = 6;

#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Session,
    Auth,
    Dashboard,
    Metrics,
    Trace,
    Editor,
    Stream,
    Objects(ObjectsSlot),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub slot: Slot,
    pub generation: u32,
}

#[derive(Default)]
pub struct SlotTable {
    generations: AHashMap<Slot, u32>,
}

impl SlotTable {
    pub fn start(&mut self, slot: Slot) -> Ticket {
        let generation = self.generations.entry(slot.clone()).or_default();
        *generation = generation.wrapping_add(1);
        Ticket {
            generation: *generation,
            slot,
        }
    }

    pub fn bump(&mut self, slot: &Slot) {
        if let Some(generation) = self.generations.get_mut(slot) {
            *generation = generation.wrapping_add(1);
        }
    }

    pub fn bump_where(&mut self, mut pred: impl FnMut(&Slot) -> bool) {
        for (slot, generation) in &mut self.generations {
            if pred(slot) {
                *generation = generation.wrapping_add(1);
            }
        }
    }

    pub fn bump_all(&mut self) {
        self.bump_where(|_| true);
    }

    pub fn is_current(&self, ticket: &Ticket) -> bool {
        self.generations.get(&ticket.slot) == Some(&ticket.generation)
    }
}

pub struct JobCx {
    pub tx: Sender<Msg>,
    pub ticket: Ticket,
}

impl JobCx {
    pub fn progress(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }

    pub fn report(&self, progress: JobProgress) {
        self.progress(Msg::Progress(self.ticket.clone(), progress));
    }
}

pub struct Executor {
    tx: Sender<Msg>,
    active: usize,
    queue: VecDeque<(Ticket, Job)>,
}

impl Executor {
    pub fn new(tx: Sender<Msg>) -> Self {
        Executor {
            tx,
            active: 0,
            queue: VecDeque::new(),
        }
    }

    pub fn submit(&mut self, ticket: Ticket, job: Job) -> io::Result<()> {
        if self.active < MAX_JOBS {
            self.spawn(ticket, job)
        } else {
            self.queue.push_back((ticket, job));
            Ok(())
        }
    }

    pub fn finished(&mut self, slots: &SlotTable) -> io::Result<()> {
        self.active = self.active.saturating_sub(1);
        while let Some((ticket, job)) = self.queue.pop_front() {
            if slots.is_current(&ticket) || !job.is_read_only() {
                return self.spawn(ticket, job);
            }
        }
        Ok(())
    }

    fn spawn(&mut self, ticket: Ticket, job: Job) -> io::Result<()> {
        let cx = JobCx {
            tx: self.tx.clone(),
            ticket,
        };
        thread::Builder::new()
            .name("tui-job".into())
            .spawn(move || {
                let msg = match panic::catch_unwind(AssertUnwindSafe(|| job.run(&cx))) {
                    Ok(output) => Msg::Done(cx.ticket.clone(), output),
                    Err(_) => Msg::JobPanicked(cx.ticket.clone()),
                };
                let _ = cx.tx.send(msg);
            })?;
        self.active += 1;
        Ok(())
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/tui/exec/tests.rs"]
mod tests;
