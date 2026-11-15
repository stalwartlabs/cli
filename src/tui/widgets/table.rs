/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::tui::theme::Theme;
use crate::view::Styled;
use ratatui::Frame;
use ratatui::layout::{Constraint, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Cell, HighlightSpacing, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
    TableState as RataState,
};
use unicode_width::UnicodeWidthChar;

pub const MAX_CELL_WIDTH: u16 = 60;
const MARK_WIDTH: u16 = 1;
const HIGHLIGHT_WIDTH: u16 = 1;
const MIN_COLUMN_WIDTH: u16 = 6;
const ELLIPSIS: &str = "…";
const COLUMN_SPACING: u16 = 2;

#[derive(Debug, Clone)]
pub struct TableColumn {
    pub label: Box<str>,
    pub width: u16,
    pub header: u16,
}

impl TableColumn {
    pub fn new(label: &str) -> Self {
        let label: Box<str> = label.to_uppercase().into_boxed_str();
        let width = text_width(&label);
        Self {
            label,
            width,
            header: width,
        }
    }

    pub fn limit(&self, cap: u16) -> u16 {
        self.width.min(cap).max(self.header.min(self.width))
    }

    pub fn fit(&mut self, cell: &Styled) {
        let width = u16::try_from(cell.width())
            .unwrap_or(u16::MAX)
            .min(MAX_CELL_WIDTH);
        self.width = self.width.max(width);
    }
}

#[derive(Debug, Clone)]
pub struct RowCells {
    pub id: Box<str>,
    pub label: Box<str>,
    pub cells: Box<[Styled]>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub selected: usize,
    pub offset: usize,
}

impl Cursor {
    pub fn move_to(&mut self, position: usize, total: usize) {
        self.selected = position.min(total.saturating_sub(1));
    }

    pub fn move_by(&mut self, delta: isize, total: usize) {
        let target = self.selected.saturating_add_signed(delta);
        self.move_to(target, total);
    }

    pub fn scroll_into_view(&mut self, height: usize) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if height > 0 && self.selected >= self.offset + height {
            self.offset = self.selected + 1 - height;
        }
    }

    pub fn visible(&self, height: usize) -> std::ops::Range<usize> {
        self.offset..self.offset + height
    }
}

pub struct TableView<'a, R, M>
where
    R: Fn(usize) -> Option<&'a RowCells>,
    M: Fn(usize) -> bool,
{
    pub columns: &'a [TableColumn],
    pub total: usize,
    pub rows: R,
    pub marked: M,
}

impl<'a, R, M> TableView<'a, R, M>
where
    R: Fn(usize) -> Option<&'a RowCells>,
    M: Fn(usize) -> bool,
{
    pub fn body_height(area: Rect) -> usize {
        usize::from(area.height.saturating_sub(1))
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme, cursor: &mut Cursor) {
        let height = Self::body_height(area);
        cursor.scroll_into_view(height);
        let header_style = Style::new().fg(theme.accent).add_modifier(Modifier::BOLD);
        let header = Row::new(
            std::iter::once(Cell::default())
                .chain(self.columns.iter().map(|c| Cell::from(&*c.label))),
        )
        .style(header_style);
        let mark = Style::new().fg(theme.accent);
        let available = area.width.saturating_sub(
            MARK_WIDTH
                + HIGHLIGHT_WIDTH
                + COLUMN_SPACING
                    .saturating_mul(u16::try_from(self.columns.len()).unwrap_or(u16::MAX)),
        );
        let caps = ColumnCaps::fit(self.columns, available);
        let end = cursor.offset.saturating_add(height).min(self.total);
        let body = (cursor.offset..end).map(|pos| {
            let marked = (self.marked)(pos);
            let mark_cell = Cell::from(Span::styled(if marked { "●" } else { " " }, mark));
            match (self.rows)(pos) {
                Some(row) => {
                    let style = if marked { mark } else { theme.text() };
                    Row::new(std::iter::once(mark_cell).chain(
                        row.cells.iter().zip(self.columns).enumerate().map(
                            |(index, (cell, column))| {
                                let limit = usize::from(column.limit(caps.of(index)));
                                Cell::from(if cell.width() > limit {
                                    clip(theme.spans(cell), limit)
                                } else {
                                    Line::from_iter(theme.spans(cell))
                                })
                            },
                        ),
                    ))
                    .style(style)
                }
                None => Row::new([mark_cell, Cell::from(Span::styled("…", theme.muted()))]),
            }
        });
        let widths = std::iter::once(Constraint::Length(MARK_WIDTH)).chain(
            self.columns.iter().enumerate().map(|(i, c)| {
                if i + 1 == self.columns.len() {
                    Constraint::Fill(1)
                } else {
                    Constraint::Length(c.limit(caps.of(i)))
                }
            }),
        );
        let mut state = RataState::default().with_offset(0).with_selected(
            (self.total > 0 && cursor.selected >= cursor.offset)
                .then(|| cursor.selected - cursor.offset),
        );
        let table = Table::new(body, widths)
            .header(header)
            .row_highlight_style(theme.selected)
            .highlight_symbol(Line::styled("▌", Style::new().fg(theme.brand)))
            .highlight_spacing(HighlightSpacing::Always)
            .column_spacing(COLUMN_SPACING);
        frame.render_stateful_widget(table, area, &mut state);
        if self.total > height {
            let mut bar = ScrollbarState::new(self.total)
                .position(cursor.selected)
                .viewport_content_length(height);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .end_symbol(None)
                    .track_symbol(Some("│"))
                    .thumb_symbol("┃")
                    .style(Style::new().fg(theme.border))
                    .thumb_style(Style::new().fg(theme.brand)),
                area.outer(Margin::new(1, 0)).inner(Margin::new(0, 1)),
                &mut bar,
            );
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnCaps {
    first: u16,
    rest: u16,
}

impl ColumnCaps {
    pub fn fit(columns: &[TableColumn], available: u16) -> Self {
        let Some((first, rest)) = columns.split_first() else {
            return ColumnCaps {
                first: u16::MAX,
                rest: u16::MAX,
            };
        };
        let rest_room = available.saturating_sub(first.width);
        let rest_min: u32 = rest
            .iter()
            .map(|c| u32::from(c.limit(MIN_COLUMN_WIDTH)))
            .sum();
        if rest_min <= u32::from(rest_room) {
            ColumnCaps {
                first: u16::MAX,
                rest: width_cap(rest, rest_room),
            }
        } else {
            let cap = width_cap(columns, available);
            ColumnCaps {
                first: cap,
                rest: cap,
            }
        }
    }

    pub fn of(self, index: usize) -> u16 {
        if index == 0 { self.first } else { self.rest }
    }
}

pub fn width_cap(columns: &[TableColumn], available: u16) -> u16 {
    let total = |cap: u16| -> u32 { columns.iter().map(|c| u32::from(c.limit(cap))).sum() };
    let widest = columns.iter().map(|c| c.width).max().unwrap_or(0);
    if total(widest) <= u32::from(available) {
        return u16::MAX;
    }
    let (mut low, mut high) = (MIN_COLUMN_WIDTH, widest);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if total(mid) <= u32::from(available) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

pub fn clip<'a>(spans: impl Iterator<Item = Span<'a>>, width: usize) -> Line<'a> {
    let mut line = Line::default();
    let mut used = 0;
    for span in spans {
        let span_width = span.width();
        if used + span_width <= width {
            used += span_width;
            line.spans.push(span);
            continue;
        }
        let room = width.saturating_sub(used + 1);
        let mut cut = String::with_capacity(span.content.len());
        let mut cut_width = 0;
        for ch in span.content.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if cut_width + ch_width > room {
                break;
            }
            cut_width += ch_width;
            cut.push(ch);
        }
        line.spans.push(Span::styled(cut, span.style));
        line.spans.push(Span::styled(ELLIPSIS, span.style));
        break;
    }
    line
}

pub fn text_width(text: &str) -> u16 {
    u16::try_from(unicode_width::UnicodeWidthStr::width(text)).unwrap_or(u16::MAX)
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/widgets/table/tests.rs"]
mod tests;
