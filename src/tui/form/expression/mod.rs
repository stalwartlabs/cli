/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod render;

use crate::schema::expression::{Expression, Part, Rule};
use crate::tui::form::editor::textarea::TextArea;
use crate::tui::form::model::Rules;
use crate::tui::form::view::{EditAction, FormAction};
use ratatui::crossterm::event::KeyEvent;
use serde_json::Value;
use std::cmp::Ordering;

pub const NOT_A_RULE: &str = "The default value cannot be removed; x clears it";
pub const READ_ONLY: &str = "This expression is read-only";

#[derive(Debug)]
pub struct ExpressionView {
    pub expression: Expression<String>,
    pub cursor: Part,
    pub status: Option<&'static str>,
    pub problem: Option<Problem>,
    original: Expression<String>,
    editing: Option<TextArea>,
    fresh: Option<usize>,
    rules: Rules,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub at: Option<Part>,
    pub message: Box<str>,
}

pub enum ExpressionStep {
    None,
    Back,
    Save,
}

impl ExpressionView {
    pub fn new(value: Option<&Value>, error: Option<&str>, rules: Rules) -> Self {
        let expression = value
            .and_then(Expression::parse)
            .map(|expression| expression.owned())
            .unwrap_or_default();
        let problem = error.map(|text| {
            let (at, message) = match Part::locate(text) {
                Some((part, message)) => (Some(part), message),
                None => (expression.empty_part(), text),
            };
            Problem {
                at: at.filter(|part| expression.text(*part).is_some()),
                message: message.into(),
            }
        });
        let mut view = Self {
            original: expression.clone(),
            expression,
            cursor: Part::Default,
            status: None,
            problem,
            editing: None,
            fresh: None,
            rules,
        };
        view.cursor = view
            .problem
            .as_ref()
            .and_then(|problem| problem.at)
            .unwrap_or_else(|| view.first());
        view
    }

    pub fn is_read_only(&self) -> bool {
        self.rules.read_only
    }

    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    pub fn editor(&mut self) -> Option<&mut TextArea> {
        self.editing.as_mut()
    }

    pub fn changed(&self) -> bool {
        self.expression != self.original
    }

    pub fn into_value(self) -> Value {
        self.expression.into_value()
    }

    pub fn copy_text(&self) -> Option<String> {
        self.expression
            .text(self.cursor)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    }

    fn first(&self) -> Part {
        if self.expression.rules.is_empty() {
            Part::Default
        } else {
            Part::Condition(0)
        }
    }

    fn next(&self, part: Part) -> Part {
        let len = self.expression.rules.len();
        match part {
            Part::Condition(index) => Part::Result(index),
            Part::Result(index) if index + 1 < len => Part::Condition(index + 1),
            Part::Result(_) | Part::Default => Part::Default,
        }
    }

    fn prev(&self, part: Part) -> Part {
        let len = self.expression.rules.len();
        match part {
            Part::Condition(0) => Part::Condition(0),
            Part::Condition(index) => Part::Result(index - 1),
            Part::Result(index) => Part::Condition(index),
            Part::Default if len > 0 => Part::Result(len - 1),
            Part::Default => Part::Default,
        }
    }

    fn next_rule(&self, ends: Ends) -> Part {
        let len = self.expression.rules.len();
        match (self.cursor.rule(), ends) {
            (Some(index), _) if index + 1 < len => Part::Condition(index + 1),
            (Some(_), _) | (None, Ends::Clamp) => Part::Default,
            (None, Ends::Wrap) => self.first(),
        }
    }

    fn prev_rule(&self, ends: Ends) -> Part {
        let len = self.expression.rules.len();
        match (self.cursor.rule(), ends) {
            (Some(index), _) if index > 0 => Part::Condition(index - 1),
            (Some(_), Ends::Clamp) => Part::Condition(0),
            (Some(_), Ends::Wrap) => Part::Default,
            (None, _) if len > 0 => Part::Condition(len - 1),
            (None, _) => Part::Default,
        }
    }

    pub fn act(&mut self, action: FormAction) -> ExpressionStep {
        self.status = None;
        let mutating = matches!(
            action,
            FormAction::Edit
                | FormAction::Add
                | FormAction::Delete
                | FormAction::Clear
                | FormAction::Revert
                | FormAction::MoveUp
                | FormAction::MoveDown
        );
        if self.rules.read_only && mutating {
            self.status = Some(READ_ONLY);
            return ExpressionStep::None;
        }
        match action {
            FormAction::Down => self.cursor = self.next(self.cursor),
            FormAction::Up => self.cursor = self.prev(self.cursor),
            FormAction::First => self.cursor = self.first(),
            FormAction::Last => self.cursor = Part::Default,
            FormAction::NextSection => self.cursor = self.next_rule(Ends::Wrap),
            FormAction::PrevSection => self.cursor = self.prev_rule(Ends::Wrap),
            FormAction::PageDown => self.cursor = self.next_rule(Ends::Clamp),
            FormAction::PageUp => self.cursor = self.prev_rule(Ends::Clamp),
            FormAction::Edit => self.open(),
            FormAction::Add => self.add(),
            FormAction::Delete => self.remove(),
            FormAction::Clear => {
                if let Some(text) = self.expression.text_mut(self.cursor) {
                    text.clear();
                }
            }
            FormAction::Revert => {
                self.expression = self.original.clone();
                self.problem = None;
                self.cursor = self.first();
            }
            FormAction::MoveUp => self.shift(Shift::Up),
            FormAction::MoveDown => self.shift(Shift::Down),
            FormAction::Back => return ExpressionStep::Back,
            FormAction::Save => return ExpressionStep::Save,
            FormAction::Toggle
            | FormAction::External
            | FormAction::Now
            | FormAction::Open
            | FormAction::Copy => {}
        }
        ExpressionStep::None
    }

    fn open(&mut self) {
        if let Some(text) = self.expression.text(self.cursor) {
            self.editing = Some(TextArea::new(text));
        }
    }

    fn add(&mut self) {
        let index = self
            .cursor
            .rule()
            .map_or(self.expression.rules.len(), |index| index + 1);
        self.expression.rules.insert(index, Rule::default());
        self.remap(|at| Some(if at >= index { at + 1 } else { at }));
        self.cursor = Part::Condition(index);
        self.fresh = Some(index);
        self.open();
    }

    fn remove(&mut self) {
        let Some(index) = self.cursor.rule() else {
            self.status = Some(NOT_A_RULE);
            return;
        };
        if index >= self.expression.rules.len() {
            return;
        }
        self.expression.rules.remove(index);
        self.remap(|at| match at.cmp(&index) {
            Ordering::Less => Some(at),
            Ordering::Equal => None,
            Ordering::Greater => Some(at - 1),
        });
        let len = self.expression.rules.len();
        self.cursor = if len == 0 {
            Part::Default
        } else {
            Part::Condition(index.min(len - 1))
        };
    }

    fn shift(&mut self, shift: Shift) {
        let Some(index) = self.cursor.rule() else {
            return;
        };
        let target = match shift {
            Shift::Up => index.checked_sub(1),
            Shift::Down => Some(index + 1).filter(|target| *target < self.expression.rules.len()),
        };
        let Some(target) = target else {
            return;
        };
        self.expression.rules.swap(index, target);
        self.remap(|at| {
            Some(if at == index {
                target
            } else if at == target {
                index
            } else {
                at
            })
        });
        self.cursor = match self.cursor {
            Part::Condition(_) => Part::Condition(target),
            Part::Result(_) => Part::Result(target),
            Part::Default => Part::Default,
        };
    }

    fn remap(&mut self, map: impl Fn(usize) -> Option<usize>) {
        let Some(problem) = &mut self.problem else {
            return;
        };
        let at = match problem.at {
            Some(Part::Condition(index)) => map(index).map(Part::Condition),
            Some(Part::Result(index)) => map(index).map(Part::Result),
            Some(Part::Default) | None => return,
        };
        match at {
            Some(at) => problem.at = Some(at),
            None => self.problem = None,
        }
    }

    pub fn edit_key(&mut self, event: KeyEvent, action: Option<EditAction>) {
        let Some(area) = &mut self.editing else {
            return;
        };
        match action {
            Some(EditAction::Done) => self.commit(),
            Some(EditAction::Cancel) => self.cancel(),
            Some(EditAction::Copy) => {}
            Some(EditAction::Commit) | None => {
                area.handle(event);
            }
        }
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(area) = &mut self.editing {
            area.paste(text);
        }
    }

    fn cancel(&mut self) {
        self.editing = None;
        let Some(index) = self.fresh.take() else {
            return;
        };
        if self
            .expression
            .rules
            .get(index)
            .is_some_and(|rule| rule.condition.is_empty() && rule.result.is_empty())
        {
            self.remove();
        }
    }

    fn commit(&mut self) {
        let Some(area) = self.editing.take() else {
            return;
        };
        let text = area.text();
        let text = text.trim();
        if let Some(slot) = self.expression.text_mut(self.cursor)
            && slot.trim() != text
        {
            slot.clear();
            slot.extend(text.chars().map(|c| if c == '\t' { ' ' } else { c }));
            if self
                .problem
                .as_ref()
                .is_some_and(|problem| problem.at == Some(self.cursor))
            {
                self.problem = None;
            }
        }
        let fresh = self.fresh.take();
        if let Part::Condition(index) = self.cursor
            && fresh == Some(index)
            && self
                .expression
                .rules
                .get(index)
                .is_some_and(|rule| rule.result.is_empty())
        {
            self.cursor = Part::Result(index);
            self.fresh = fresh;
            self.open();
        }
    }
}

#[derive(Clone, Copy)]
enum Ends {
    Wrap,
    Clamp,
}

#[derive(Clone, Copy)]
enum Shift {
    Up,
    Down,
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../../../cli-tests/unit/src/tui/form/expression/tests.rs"]
mod tests;
