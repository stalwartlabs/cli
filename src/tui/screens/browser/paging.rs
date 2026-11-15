/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::query::Total;
use crate::tui::widgets::table::RowCells;
use std::ops::Range;

pub const MAX_PAGES_KEPT: usize = 30;
pub const MAX_PAGE_SIZE: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    pub page: usize,
    pub position: usize,
    pub limit: usize,
    pub total: Total,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrival {
    Stored,
    PastEnd,
}

#[derive(Debug)]
struct Page {
    number: usize,
    rows: Vec<RowCells>,
}

#[derive(Debug)]
pub struct Pager {
    page_size: usize,
    total: Option<usize>,
    pages: Vec<Page>,
    pending: Vec<usize>,
}

impl Pager {
    pub fn new(max_get: usize) -> Self {
        Self {
            page_size: max_get.clamp(1, MAX_PAGE_SIZE),
            total: None,
            pages: Vec::new(),
            pending: Vec::new(),
        }
    }

    pub fn page_size(&self) -> usize {
        self.page_size
    }

    pub fn total(&self) -> Option<usize> {
        self.total
    }

    pub fn loaded(&self) -> usize {
        self.pages.iter().map(|p| p.rows.len()).sum()
    }

    pub fn reset(&mut self) {
        self.total = None;
        self.pages.clear();
        self.pending.clear();
    }

    pub fn row(&self, position: usize) -> Option<&RowCells> {
        let number = position / self.page_size;
        self.pages
            .iter()
            .find(|p| p.number == number)?
            .rows
            .get(position % self.page_size)
    }

    pub fn rows(&self) -> impl Iterator<Item = &RowCells> {
        self.pages.iter().flat_map(|p| p.rows.iter())
    }

    pub fn request(&mut self, viewport: Range<usize>, forward: bool) -> Option<PageRequest> {
        let Some(total) = self.total else {
            return self.claim(0, Total::Calculate);
        };
        if total == 0 {
            return None;
        }
        let last_page = (total - 1) / self.page_size;
        let first = viewport.start / self.page_size;
        let last = viewport.end.saturating_sub(1) / self.page_size;
        let ahead = if forward {
            last.saturating_add(1)
        } else {
            first.saturating_sub(1)
        };
        let wanted = (first..=last).chain(std::iter::once(ahead));
        let number = wanted
            .filter(|n| *n <= last_page)
            .find(|n| !self.has(*n) && !self.pending.contains(n))?;
        self.claim(number, Total::Skip)
    }

    fn claim(&mut self, number: usize, total: Total) -> Option<PageRequest> {
        if self.pending.contains(&number) {
            return None;
        }
        self.pending.push(number);
        Some(PageRequest {
            page: number,
            position: number * self.page_size,
            limit: self.page_size,
            total,
        })
    }

    fn has(&self, number: usize) -> bool {
        self.pages.iter().any(|p| p.number == number)
    }

    pub fn failed(&mut self, page: usize) {
        self.pending.retain(|n| *n != page);
    }

    pub fn arrive(
        &mut self,
        page: usize,
        rows: Vec<RowCells>,
        total: Option<usize>,
        focus: usize,
    ) -> Arrival {
        self.pending.retain(|n| *n != page);
        if let Some(total) = total {
            self.total = Some(total);
        }
        if rows.is_empty() && page > 0 {
            self.reset();
            return Arrival::PastEnd;
        }
        if self.total.is_none() {
            self.total = Some(page * self.page_size + rows.len());
        }
        self.pages.retain(|p| p.number != page);
        self.pages.push(Page { number: page, rows });
        self.evict(focus / self.page_size);
        Arrival::Stored
    }

    fn evict(&mut self, focus_page: usize) {
        while self.pages.len() > MAX_PAGES_KEPT {
            let Some(far) = self
                .pages
                .iter()
                .enumerate()
                .max_by_key(|(_, p)| p.number.abs_diff(focus_page))
                .map(|(i, _)| i)
            else {
                return;
            };
            self.pages.swap_remove(far);
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/screens/browser/paging/tests.rs"]
mod tests;
