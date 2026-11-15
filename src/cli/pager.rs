/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::io::{BufRead, Write, stderr, stdin};

use crate::error::Result;

const PAGE_SIZE: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Paging {
    Interactive,
    Stream,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageEnd {
    Last,
    NotLast,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum More {
    Continue,
    Resumed,
    Stop,
}

pub struct TtyPager {
    paging: Paging,
    shown: usize,
}

impl TtyPager {
    pub fn new(paging: Paging) -> Self {
        TtyPager { paging, shown: 0 }
    }

    pub fn after_row(&mut self, at: PageEnd) -> Result<More> {
        self.shown += 1;
        if self.row_needs_prompt(at) {
            Self::prompt()
        } else {
            Ok(More::Continue)
        }
    }

    pub fn after_page(&self) -> Result<More> {
        if self.page_needs_prompt() {
            Self::prompt()
        } else {
            Ok(More::Continue)
        }
    }

    fn row_needs_prompt(&self, at: PageEnd) -> bool {
        self.paging == Paging::Interactive
            && self.shown.is_multiple_of(PAGE_SIZE)
            && at == PageEnd::NotLast
    }

    fn page_needs_prompt(&self) -> bool {
        self.paging == Paging::Interactive
            && self.shown > 0
            && !self.shown.is_multiple_of(PAGE_SIZE)
    }

    fn prompt() -> Result<More> {
        let mut err = stderr().lock();
        err.write_all(b"Show more? [Y/n] ")?;
        err.flush()?;
        drop(err);

        let mut line = String::new();
        stdin().lock().read_line(&mut line)?;
        let stop = hashify::set_ignore_case!(line.trim().as_bytes(), "n", "no");
        Ok(if stop { More::Stop } else { More::Resumed })
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/cli/pager/tests.rs"]
mod tests;
