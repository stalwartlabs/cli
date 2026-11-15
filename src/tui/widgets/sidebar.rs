/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::{Layout, LayoutItem, LayoutSubItem};
use crate::tui::keymap::{code, key, keys};
use crate::tui::session::Access;
use crate::tui::theme::Theme;
use crate::tui::widgets::chrome::pad;
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

pub const SIDEBAR_WIDTH: u16 = 34;
pub const NARROW_WIDTH: u16 = 100;
const INDENT: usize = 2;
const LOCK_MARKER: &str = " ◆";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarAction {
    Down,
    Up,
    Open,
    NextLayout,
    PrevLayout,
    Leave,
    Top,
    Bottom,
}

keys! { pub KEYS, pub HINTS: SidebarAction;
    "j/k" Down "move" true [code(KeyCode::Down), key('j')],
    "k ↑" Up "move up" false [code(KeyCode::Up), key('k')],
    "⏎" Open "open / fold" true [code(KeyCode::Enter), key(' ')],
    "[ ]" NextLayout "switch layout" true [key(']')],
    "[" PrevLayout "previous layout" false [key('[')],
    "l →" Leave "back to content" true [code(KeyCode::Right), key('l'), code(KeyCode::Tab), code(KeyCode::Esc)],
    "g" Top "first" false [key('g'), code(KeyCode::Home)],
    "G" Bottom "last" false [key('G'), code(KeyCode::End)],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarMode {
    Always,
    Auto,
}

impl SidebarMode {
    pub fn toggled(self) -> Self {
        match self {
            SidebarMode::Always => SidebarMode::Auto,
            SidebarMode::Auto => SidebarMode::Always,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Node {
    pub depth: u8,
    pub label: Box<str>,
    pub view: Option<Box<str>>,
    pub locked: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LayoutTree {
    pub name: Box<str>,
    pub nodes: Vec<Node>,
}

impl LayoutTree {
    pub fn build(layout: &Layout, access: &mut impl FnMut(&str) -> Access) -> Self {
        let mut nodes = Vec::new();
        for item in &layout.items {
            match item {
                LayoutItem::Link { link } => {
                    push_link(&mut nodes, 0, &link.name, &link.view_name, access)
                }
                LayoutItem::Container { container } => {
                    push_container(&mut nodes, 0, &container.name, &container.items, access)
                }
                LayoutItem::Unknown(_) => {}
            }
        }
        LayoutTree {
            name: layout.name.as_str().into(),
            nodes,
        }
    }

    pub fn links(&self) -> impl Iterator<Item = (&Node, usize)> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.view.is_some())
            .map(|(i, n)| (n, i))
    }

    pub fn first_link(&self) -> Option<&str> {
        self.nodes
            .iter()
            .filter(|n| !n.locked)
            .find_map(|n| n.view.as_deref())
            .or_else(|| self.nodes.iter().find_map(|n| n.view.as_deref()))
    }

    pub fn find(&self, view: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.view.as_deref() == Some(view))
    }

    pub fn ancestors(&self, index: usize) -> impl Iterator<Item = (usize, &Node)> {
        let mut depth = self.nodes.get(index).map_or(0, |n| n.depth);
        self.nodes
            .iter()
            .enumerate()
            .take(index)
            .rev()
            .filter(move |(_, n)| {
                if n.depth < depth {
                    depth = n.depth;
                    true
                } else {
                    false
                }
            })
    }
}

fn push_link(
    nodes: &mut Vec<Node>,
    depth: u8,
    name: &str,
    view: &str,
    access: &mut impl FnMut(&str) -> Access,
) {
    let locked = match access(view) {
        Access::Hidden => return,
        Access::Locked => true,
        Access::Open => false,
    };
    nodes.push(Node {
        depth,
        label: name.into(),
        view: Some(view.into()),
        locked,
    });
}

fn push_container(
    nodes: &mut Vec<Node>,
    depth: u8,
    name: &str,
    items: &[LayoutSubItem],
    access: &mut impl FnMut(&str) -> Access,
) {
    let header = nodes.len();
    nodes.push(Node {
        depth,
        label: name.into(),
        view: None,
        locked: false,
    });
    let child = depth.saturating_add(1);
    for item in items {
        match item {
            LayoutSubItem::Link { name, view_name } => {
                push_link(nodes, child, name, view_name, access)
            }
            LayoutSubItem::Container { name, items } => {
                push_container(nodes, child, name, items, access)
            }
            LayoutSubItem::Unknown => {}
        }
    }
    if nodes.len() == header + 1 {
        nodes.truncate(header);
    }
}

pub enum Outcome {
    Stay,
    Leave,
    Open { layout: usize, view: Box<str> },
    Switch(usize),
}

pub struct Sidebar {
    pub mode: SidebarMode,
    pub layout: usize,
    selected: usize,
    closed: Vec<Vec<bool>>,
    rows: Vec<usize>,
}

impl Sidebar {
    pub fn new(mode: SidebarMode) -> Self {
        Sidebar {
            mode,
            layout: 0,
            selected: 0,
            closed: Vec::new(),
            rows: Vec::new(),
        }
    }

    pub fn reset(&mut self, trees: &[LayoutTree]) {
        self.closed = trees
            .iter()
            .map(|t| t.nodes.iter().map(|n| n.view.is_none()).collect())
            .collect();
        self.layout = 0;
        self.selected = 0;
        self.refresh(trees);
    }

    pub fn shown(&self, focused: bool, width: u16) -> bool {
        focused || (self.mode == SidebarMode::Always && width >= NARROW_WIDTH)
    }

    pub fn toggle_mode(&mut self) -> &'static str {
        self.mode = self.mode.toggled();
        match self.mode {
            SidebarMode::Always => "sidebar: always shown",
            SidebarMode::Auto => "sidebar: auto-hide (← brings it back)",
        }
    }

    fn refresh(&mut self, trees: &[LayoutTree]) {
        self.rows.clear();
        let (Some(tree), Some(closed)) = (trees.get(self.layout), self.closed.get(self.layout))
        else {
            return;
        };
        let mut hide_below: Option<u8> = None;
        for (i, node) in tree.nodes.iter().enumerate() {
            if let Some(depth) = hide_below {
                if node.depth > depth {
                    continue;
                }
                hide_below = None;
            }
            self.rows.push(i);
            if closed.get(i).copied().unwrap_or(false) {
                hide_below = Some(node.depth);
            }
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    pub fn select_view(&mut self, trees: &[LayoutTree], layout: usize, view: &str) {
        let Some(tree) = trees.get(layout) else {
            return;
        };
        let Some(index) = tree
            .nodes
            .iter()
            .position(|n| n.view.as_deref() == Some(view))
        else {
            return;
        };
        self.layout = layout;
        if let Some(closed) = self.closed.get_mut(layout) {
            for (i, _) in tree.ancestors(index) {
                if let Some(flag) = closed.get_mut(i) {
                    *flag = false;
                }
            }
        }
        self.refresh(trees);
        if let Some(row) = self.rows.iter().position(|&i| i == index) {
            self.selected = row;
        }
    }

    pub fn handle(&mut self, trees: &[LayoutTree], action: SidebarAction) -> Outcome {
        let count = trees.len().max(1);
        match action {
            SidebarAction::Down => {
                self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1))
            }
            SidebarAction::Up => self.selected = self.selected.saturating_sub(1),
            SidebarAction::Top => self.selected = 0,
            SidebarAction::Bottom => self.selected = self.rows.len().saturating_sub(1),
            SidebarAction::NextLayout => return self.switch(trees, (self.layout + 1) % count),
            SidebarAction::PrevLayout => {
                return self.switch(trees, (self.layout + count - 1) % count);
            }
            SidebarAction::Leave => return Outcome::Leave,
            SidebarAction::Open => {
                let Some(&index) = self.rows.get(self.selected) else {
                    return Outcome::Stay;
                };
                let Some(node) = trees.get(self.layout).and_then(|t| t.nodes.get(index)) else {
                    return Outcome::Stay;
                };
                if let Some(view) = &node.view {
                    return Outcome::Open {
                        layout: self.layout,
                        view: view.clone(),
                    };
                }
                if let Some(flag) = self
                    .closed
                    .get_mut(self.layout)
                    .and_then(|c| c.get_mut(index))
                {
                    *flag = !*flag;
                }
                self.refresh(trees);
            }
        }
        Outcome::Stay
    }

    fn switch(&mut self, trees: &[LayoutTree], layout: usize) -> Outcome {
        if layout == self.layout {
            return Outcome::Stay;
        }
        self.layout = layout;
        self.selected = 0;
        self.refresh(trees);
        Outcome::Switch(layout)
    }

    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        trees: &[LayoutTree],
        current: Option<&str>,
        focused: bool,
    ) {
        let Some(tree) = trees.get(self.layout) else {
            return;
        };
        let closed = self.closed.get(self.layout).map_or(&[][..], Vec::as_slice);
        let mut switcher = Line::from(Span::raw(" "));
        for (i, t) in trees.iter().enumerate() {
            if i > 0 {
                switcher.push_span(Span::styled(" · ", theme.muted()));
            }
            let style = if i == self.layout {
                theme.strong()
            } else {
                theme.muted()
            };
            switcher.push_span(Span::styled(t.name.as_ref(), style));
        }
        let mut title = Line::from(vec![
            Span::raw(" "),
            Span::raw(tree.name.as_ref()),
            Span::raw(" "),
        ]);
        if focused {
            title = title.style(theme.strong());
        }
        let block = theme.panel(title, focused).title_bottom(switcher);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let width = usize::from(inner.width).saturating_sub(1);
        let height = usize::from(inner.height);
        let skip = self.selected.saturating_sub(height.saturating_sub(2));
        let mut lines = Vec::with_capacity(height.min(self.rows.len()));
        for (row, &i) in self.rows.iter().enumerate().skip(skip).take(height) {
            let Some(node) = tree.nodes.get(i) else {
                continue;
            };
            let marker = match &node.view {
                None if closed.get(i).copied().unwrap_or(false) => "▸ ",
                None => "▾ ",
                Some(_) => "  ",
            };
            let is_current = node.view.is_some() && node.view.as_deref() == current;
            let mut style = match &node.view {
                None => theme.title(),
                Some(_) if is_current => theme.strong(),
                Some(_) if node.locked => theme.muted(),
                Some(_) => theme.text(),
            };
            let lead = if focused && row == self.selected {
                style = theme.selected;
                "▶"
            } else if is_current {
                "▌"
            } else {
                " "
            };
            let indent = usize::from(node.depth) * INDENT;
            let lock = if node.locked { LOCK_MARKER } else { "" };
            let used = indent + 2 + node.label.width() + lock.width();
            lines.push(Line::from(vec![
                Span::styled(lead, Style::new().fg(theme.brand)),
                Span::styled(pad(indent), style),
                Span::styled(marker, style),
                Span::styled(node.label.as_ref(), style),
                Span::styled(lock, style.fg(theme.accent)),
                Span::styled(pad(width.saturating_sub(used)), style),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../cli-tests/unit/src/tui/widgets/sidebar/tests.rs"]
mod tests;
