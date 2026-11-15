/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::http::HTTP_TRACE;
use log::{LevelFilter, Log, Metadata, Record};
use std::collections::VecDeque;
use std::fmt::Write;
use std::sync::{Arc, Mutex, MutexGuard};

pub const MAX_LINES: usize = 500;

#[derive(Clone, Default)]
pub struct HttpLog(Arc<Mutex<Ring>>);

#[derive(Default)]
struct Ring {
    lines: VecDeque<Box<str>>,
    pushed: u64,
}

impl HttpLog {
    fn lock(&self) -> MutexGuard<'_, Ring> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn push(&self, line: Box<str>) {
        let mut ring = self.lock();
        if ring.lines.len() == MAX_LINES {
            ring.lines.pop_front();
        }
        ring.lines.push_back(line);
        ring.pushed = ring.pushed.wrapping_add(1);
    }

    pub fn copy_new(&self, seen: &mut u64, out: &mut VecDeque<Box<str>>) -> bool {
        let ring = self.lock();
        let fresh = ring.pushed.wrapping_sub(*seen);
        if fresh == 0 {
            return false;
        }
        *seen = ring.pushed;
        let fresh = usize::try_from(fresh).unwrap_or(usize::MAX);
        match ring.lines.len().checked_sub(fresh) {
            Some(skip) if skip > 0 => {
                out.extend(ring.lines.iter().skip(skip).cloned());
                let excess = out.len().saturating_sub(MAX_LINES);
                out.drain(..excess);
            }
            _ => {
                out.clear();
                out.extend(ring.lines.iter().cloned());
            }
        }
        true
    }

    pub fn install(&self) -> bool {
        let logger = HttpLogger { log: self.clone() };
        if log::set_logger(Box::leak(Box::new(logger))).is_ok() {
            log::set_max_level(LevelFilter::Debug);
            true
        } else {
            false
        }
    }
}

struct HttpLogger {
    log: HttpLog,
}

impl Log for HttpLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target() == HTTP_TRACE
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let mut line = String::new();
            let _ = write!(line, "{}", record.args());
            self.log.push(line.into_boxed_str());
        }
    }

    fn flush(&self) {}
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/tui/log/tests.rs"]
mod tests;
