/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::{DashboardItem, ItemKind};
use ratatui::layout::Rect;
use std::mem;

pub const COLUMNS: u16 = 12;
const MAX_ROWS: u16 = 2;
const STAT_SPAN: u16 = 3;
const CHART_SPAN: u16 = 6;
const PANEL_SPAN: u16 = 4;
const SMALL_HEIGHT: u16 = 5;
const CHART_HEIGHT: u16 = 6;
const RANKING_HEIGHT: u16 = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub row: u16,
    pub col: u16,
    pub span: u16,
    pub rows: u16,
    pub min: u16,
    pub grows: bool,
}

impl Placed {
    fn covers(&self, row: usize) -> bool {
        (usize::from(self.row)..usize::from(self.row + self.rows)).contains(&row)
    }
}

pub fn place(items: &[DashboardItem], out: &mut Vec<Placed>) {
    out.clear();
    out.reserve(items.len());
    let mut occupied: Vec<u16> = Vec::new();
    for item in items {
        let (span, min, grows) = match &item.kind {
            ItemKind::Stat(_) | ItemKind::Unknown => (STAT_SPAN, SMALL_HEIGHT, false),
            ItemKind::Chart(_) => (CHART_SPAN, CHART_HEIGHT, true),
            ItemKind::Headline(_) => (PANEL_SPAN, SMALL_HEIGHT, false),
            ItemKind::Ranking(_) => (PANEL_SPAN, RANKING_HEIGHT, false),
        };
        let span = item
            .placement
            .span
            .map_or(span, u16::from)
            .clamp(1, COLUMNS);
        let rows = item.placement.rows.map_or(1, u16::from).clamp(1, MAX_ROWS);
        let mask = ((1u32 << span) - 1) as u16;
        let mut row = 0usize;
        let (row, col) = loop {
            let end = row + usize::from(rows);
            if occupied.len() < end {
                occupied.resize(end, 0);
            }
            let window = occupied.get(row..end).unwrap_or_default();
            let free =
                (0..=COLUMNS - span).find(|&col| window.iter().all(|r| r & (mask << col) == 0));
            if let Some(col) = free {
                break (row, col);
            }
            row += 1;
        };
        for cells in occupied.iter_mut().skip(row).take(usize::from(rows)) {
            *cells |= mask << col;
        }
        out.push(Placed {
            row: u16::try_from(row).unwrap_or(u16::MAX),
            col,
            span,
            rows,
            min,
            grows,
        });
    }
}

pub fn layout(placed: &[Placed], width: u16, height: u16, out: &mut Vec<Rect>) -> u16 {
    out.clear();
    let count = placed
        .iter()
        .map(|p| usize::from(p.row + p.rows))
        .max()
        .unwrap_or(0);
    let mut heights = vec![0u16; count];
    for p in placed.iter().filter(|p| p.rows == 1) {
        if let Some(h) = heights.get_mut(usize::from(p.row)) {
            *h = (*h).max(p.min);
        }
    }
    for p in placed.iter().filter(|p| p.rows > 1) {
        let rows = usize::from(p.row)..usize::from(p.row + p.rows);
        let have: u16 = heights.get(rows.clone()).unwrap_or_default().iter().sum();
        if let Some(last) = heights.get_mut(rows.end - 1) {
            *last += p.min.saturating_sub(have);
        }
    }
    let tall = placed.iter().any(|p| p.rows > 1);
    let grows: Vec<bool> = (0..count)
        .map(|row| {
            placed
                .iter()
                .any(|p| p.covers(row) && if tall { p.rows > 1 } else { p.grows })
        })
        .collect();
    let used: u16 = heights.iter().sum();
    let growing = u16::try_from(grows.iter().filter(|g| **g).count()).unwrap_or(u16::MAX);
    let spare = height.saturating_sub(used);
    if let (Some(share), Some(mut extra)) = (spare.checked_div(growing), spare.checked_rem(growing))
    {
        for (h, _) in heights.iter_mut().zip(&grows).filter(|(_, g)| **g).rev() {
            *h += share + mem::take(&mut extra);
        }
    }
    let mut tops = Vec::with_capacity(count + 1);
    let mut y = 0u16;
    for h in &heights {
        tops.push(y);
        y = y.saturating_add(*h);
    }
    tops.push(y);
    let col_x = |c: u16| {
        u16::try_from(u32::from(width) * u32::from(c) / u32::from(COLUMNS)).unwrap_or(width)
    };
    out.extend(placed.iter().map(|p| {
        let top = tops.get(usize::from(p.row)).copied().unwrap_or(0);
        let bottom = tops
            .get(usize::from(p.row + p.rows))
            .copied()
            .unwrap_or(top);
        Rect::new(
            col_x(p.col),
            top,
            col_x(p.col + p.span) - col_x(p.col),
            bottom - top,
        )
    }));
    y
}

pub fn neighbour(rects: &[Rect], focus: usize, dx: i32, dy: i32) -> Option<usize> {
    let current = center(*rects.get(focus)?);
    rects
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != focus)
        .filter_map(|(i, rect)| {
            let (x, y) = center(*rect);
            let (ddx, ddy) = (x - current.0, y - current.1);
            let along = ddx * dx + ddy * dy;
            let across = (ddx * dy).abs() + (ddy * dx).abs();
            (along > 0).then_some((along + across * 3, i))
        })
        .min()
        .map(|(_, i)| i)
}

fn center(rect: Rect) -> (i32, i32) {
    (
        i32::from(rect.x) + i32::from(rect.width) / 2,
        (i32::from(rect.y) + i32::from(rect.height) / 2) * 2,
    )
}
