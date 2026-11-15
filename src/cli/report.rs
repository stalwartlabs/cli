/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::message::CliMessage;
use crate::cli::term::Ansi;
use crate::plan::{
    ApplyEvent, BatchAction, OpKind, PlanStats, SnapshotEvent, SnapshotStats, Summary,
};
use crate::schema::object::display_name;
use serde::Serialize;
use std::fmt;
use std::io::Write;
use std::ops::ControlFlow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    Normal,
    Quiet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Hidden,
    Shown,
}

struct Lines<'w> {
    verbosity: Verbosity,
    err: &'w mut dyn Write,
}

impl Lines<'_> {
    fn note(&mut self, args: fmt::Arguments<'_>) {
        if self.verbosity == Verbosity::Normal {
            self.always(args);
        }
    }

    fn always(&mut self, args: fmt::Arguments<'_>) {
        let _ = self.err.write_fmt(args);
        let _ = self.err.write_all(b"\n");
    }
}

struct JsonLines<'w> {
    out: &'w mut dyn Write,
}

impl JsonLines<'_> {
    fn record(&mut self, record: &impl Serialize) {
        if serde_json::to_writer(&mut *self.out, record).is_ok() {
            let _ = self.out.write_all(b"\n");
        }
    }
}

#[derive(Serialize)]
struct OpRecord<'a> {
    op: &'static str,
    object: &'a str,
    index: usize,
    count: usize,
    status: &'static str,
}

#[derive(Serialize)]
struct CleanupRecord<'a> {
    op: &'static str,
    stage: &'static str,
    object: &'a str,
    index: usize,
    destroyed: usize,
    status: &'static str,
}

#[derive(Serialize)]
struct ErrorRecord<'a> {
    op: &'static str,
    object: &'a str,
    index: usize,
    status: &'static str,
    error: String,
}

#[derive(Serialize)]
struct DoneCounts {
    destroyed: usize,
    updated: usize,
    created: usize,
    failed: usize,
}

#[derive(Serialize)]
struct SummaryRecord<'a> {
    op: &'static str,
    plan: &'a PlanStats,
    done: DoneCounts,
}

const STATUS_OK: &str = "ok";

struct Count {
    count: usize,
    one: &'static str,
    many: &'static str,
}

impl Count {
    const fn new(count: usize, one: &'static str, many: &'static str) -> Self {
        Count { count, one, many }
    }
}

impl fmt::Display for Count {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let noun = if self.count == 1 { self.one } else { self.many };
        write!(f, "{} {noun}", self.count)
    }
}

struct NonZero<'a> {
    counts: &'a [Count],
    empty: &'static str,
}

impl fmt::Display for NonZero<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut shown = self.counts.iter().filter(|c| c.count > 0);
        let Some(first) = shown.next() else {
            return f.write_str(self.empty);
        };
        write!(f, "{first}")?;
        for count in shown {
            write!(f, ", {count}")?;
        }
        Ok(())
    }
}

pub struct ApplyReporter<'w> {
    ansi: Ansi,
    format: OutputFormat,
    progress: Progress,
    lines: Lines<'w>,
    json: JsonLines<'w>,
}

impl<'w> ApplyReporter<'w> {
    pub fn new(
        ansi: Ansi,
        verbosity: Verbosity,
        format: OutputFormat,
        progress: Progress,
        out: &'w mut dyn Write,
        err: &'w mut dyn Write,
    ) -> Self {
        ApplyReporter {
            ansi,
            format,
            progress,
            lines: Lines { verbosity, err },
            json: JsonLines { out },
        }
    }

    pub fn plan_header(&mut self, stats: &PlanStats) {
        let a = self.ansi;
        let ops = [
            Count::new(stats.destroys, "destroy", "destroys"),
            Count::new(stats.updates, "update", "updates"),
            Count::new(stats.creates, "create", "creates"),
            Count::new(stats.upserts, "upsert", "upserts"),
            Count::new(stats.reconciles, "reconcile", "reconciles"),
        ];
        self.lines.note(format_args!(
            "{}Plan:{} {} ({})",
            a.bold(),
            a.reset(),
            NonZero {
                counts: &ops,
                empty: "no operations",
            },
            Count::new(stats.objects(), "object", "objects"),
        ));
    }

    pub fn dry_run_note(&mut self) {
        let a = self.ansi;
        self.lines.note(format_args!(
            "{}Dry run: no changes will be made.{}",
            a.dim(),
            a.reset()
        ));
    }

    pub fn event(&mut self, event: ApplyEvent<'_>) -> ControlFlow<()> {
        let a = self.ansi;
        match event {
            ApplyEvent::ValueMatchFallback { object } => self.lines.note(format_args!(
                "{} {} has no match key; matching existing objects by value (a changed \
                 object will be created as a new one)",
                a.warning_tag(),
                display_name(object),
            )),
            ApplyEvent::Batch {
                action,
                object,
                number,
                of,
                size,
            } => {
                if self.progress == Progress::Shown {
                    let (verb, unit) = match action {
                        BatchAction::Create => ("Creating", Count::new(size, "object", "objects")),
                        BatchAction::Update => ("Updating", Count::new(size, "object", "objects")),
                        BatchAction::Destroy => ("Destroying", Count::new(size, "id", "ids")),
                    };
                    self.lines.note(format_args!(
                        "{}·{} {verb} {} batch {number}/{of} ({unit})",
                        a.dim(),
                        a.reset(),
                        display_name(object),
                    ));
                }
            }
            ApplyEvent::OpDone {
                index,
                kind,
                object,
                count,
            } => match self.format {
                OutputFormat::Json => self.json.record(&OpRecord {
                    op: kind.as_str(),
                    object: display_name(object),
                    index,
                    count,
                    status: STATUS_OK,
                }),
                OutputFormat::Human => self.lines.note(format_args!(
                    "{}✓{} {} {} ({})",
                    a.green(),
                    a.reset(),
                    kind.past_tense(),
                    display_name(object),
                    Count::new(count, "object", "objects"),
                )),
            },
            ApplyEvent::ReconcileCleanup {
                index,
                object,
                destroyed,
            } => match self.format {
                OutputFormat::Json => self.json.record(&CleanupRecord {
                    op: OpKind::Reconcile.as_str(),
                    stage: "cleanup",
                    object: display_name(object),
                    index,
                    destroyed,
                    status: STATUS_OK,
                }),
                OutputFormat::Human if destroyed > 0 => self.lines.note(format_args!(
                    "{}✓{} Removed {} not in the plan ({})",
                    a.green(),
                    a.reset(),
                    display_name(object),
                    Count::new(destroyed, "object", "objects"),
                )),
                OutputFormat::Human => {}
            },
            ApplyEvent::OpFailed {
                index,
                kind,
                object,
                error,
            } => match self.format {
                OutputFormat::Json => self.json.record(&ErrorRecord {
                    op: kind.as_str(),
                    object: display_name(object),
                    index,
                    status: "error",
                    error: CliMessage(error).to_string(),
                }),
                OutputFormat::Human => self.lines.always(format_args!(
                    "{}✗{} {} {} failed: {}",
                    a.red(),
                    a.reset(),
                    kind.label(),
                    display_name(object),
                    CliMessage(error),
                )),
            },
            ApplyEvent::Finished(summary) => self.summary(summary),
        }
        ControlFlow::Continue(())
    }

    fn summary(&mut self, s: &Summary) {
        match self.format {
            OutputFormat::Json => self.json.record(&SummaryRecord {
                op: "summary",
                plan: &s.planned,
                done: DoneCounts {
                    destroyed: s.destroyed,
                    updated: s.updated,
                    created: s.created,
                    failed: s.failed,
                },
            }),
            OutputFormat::Human => {
                let a = self.ansi;
                let done = [
                    Count::new(s.destroyed, "destroyed", "destroyed"),
                    Count::new(s.updated, "updated", "updated"),
                    Count::new(s.created, "created", "created"),
                ];
                let done = NonZero {
                    counts: &done,
                    empty: "no changes",
                };
                if s.failed > 0 {
                    self.lines.always(format_args!(
                        "{}Done:{} {done}, {}{} failed{}",
                        a.bold(),
                        a.reset(),
                        a.red(),
                        s.failed,
                        a.reset(),
                    ))
                } else {
                    self.lines
                        .always(format_args!("{}Done:{} {done}", a.bold(), a.reset()))
                }
            }
        }
    }
}

pub struct SnapshotReporter<'w> {
    ansi: Ansi,
    lines: Lines<'w>,
}

impl<'w> SnapshotReporter<'w> {
    pub fn new(ansi: Ansi, verbosity: Verbosity, err: &'w mut dyn Write) -> Self {
        SnapshotReporter {
            ansi,
            lines: Lines { verbosity, err },
        }
    }

    pub fn plan_header(&mut self, stats: SnapshotStats) {
        let a = self.ansi;
        let counts = [
            Count::new(stats.upserts, "upsert", "upserts"),
            Count::new(stats.singletons, "singleton update", "singleton updates"),
        ];
        self.lines.note(format_args!(
            "{}Snapshot:{} {}",
            a.bold(),
            a.reset(),
            NonZero {
                counts: &counts,
                empty: "nothing to capture",
            }
        ));
    }

    pub fn done(&mut self) {
        let a = self.ansi;
        self.lines.note(format_args!(
            "{}✓{} Snapshot complete",
            a.green(),
            a.reset()
        ));
    }

    pub fn event(&mut self, event: SnapshotEvent<'_>) -> ControlFlow<()> {
        let a = self.ansi;
        match event {
            SnapshotEvent::ValueMatchFallback { object } => self.lines.note(format_args!(
                "  {} {} has no label property; objects will be matched by value on apply \
                 (a changed object will be created as a new one). Add a `matchOn` to the plan to \
                 match on a specific key.",
                a.warning_tag(),
                display_name(object)
            )),
            SnapshotEvent::AnonymizedSecrets { object } => self.lines.note(format_args!(
                "  {} {} has secret field(s) the server returns anonymized (****); their \
                 values cannot be captured and must be supplied in the plan before applying.",
                a.warning_tag(),
                display_name(object)
            )),
            SnapshotEvent::Fetching { object } => self
                .lines
                .note(format_args!("  Fetching {}…", display_name(object))),
            SnapshotEvent::Fetched { count } => self.lines.note(format_args!(
                "    {}{} fetched{}",
                a.dim(),
                Count::new(count, "object", "objects"),
                a.reset()
            )),
            SnapshotEvent::ShardDone {
                object,
                variant: Some(variant),
                count,
            } => self.lines.note(format_args!(
                "    {} {variant}: {}",
                display_name(object),
                Count::new(count, "object", "objects"),
            )),
            SnapshotEvent::ShardDone { variant: None, .. } => {}
            SnapshotEvent::FetchingSingleton { object } => self.lines.note(format_args!(
                "  Fetching {} (singleton)…",
                display_name(object)
            )),
        }
        ControlFlow::Continue(())
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/cli/report/tests.rs"]
mod tests;
