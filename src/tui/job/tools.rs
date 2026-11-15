/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::cli::report::{ApplyReporter, OutputFormat, Progress, SnapshotReporter, Verbosity};
use crate::cli::term::{Ansi, ColorMode};
use crate::error::{Error, Result};
use crate::jmap::Client;
use crate::jmap::cache::tmp_path;
use crate::plan::{ApplyError, ApplyOptions, OnError, Plan, Secrets, Snapshot, SnapshotError};
use crate::schema::Schema;
use crate::tui::exec::{Cancel, JobCx};
use crate::tui::msg::JobProgress;
use std::borrow::Cow;
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::AtomicU64;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[cfg(unix)]
const PRIVATE_MODE: u32 = 0o600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyMode {
    DryRun,
    Run(OnError),
}

pub struct ApplySpec {
    pub path: PathBuf,
    pub mode: ApplyMode,
    pub cancel: Cancel,
}

pub struct SnapshotSpec {
    pub objects: Vec<String>,
    pub path: PathBuf,
    pub secrets: Secrets,
    pub cancel: Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    Failed,
    Cancelled,
}

struct LiveLog<'a> {
    text: Vec<u8>,
    sent: usize,
    cx: &'a JobCx,
}

#[derive(Debug)]
pub struct ToolReport {
    pub log: Box<str>,
    pub outcome: Outcome,
}

pub fn apply(client: &Client, schema: &Schema, spec: &ApplySpec, cx: &JobCx) -> ToolReport {
    let mut log = LiveLog::new(cx);
    let result = apply_into(client, schema, spec, &mut log);
    report(log.text, result)
}

pub fn snapshot(client: &Client, schema: &Schema, spec: &SnapshotSpec, cx: &JobCx) -> ToolReport {
    let mut log = LiveLog::new(cx);
    let result = snapshot_into(client, schema, spec, &mut log);
    report(log.text, result)
}

impl<'a> LiveLog<'a> {
    fn new(cx: &'a JobCx) -> Self {
        LiveLog {
            text: Vec::new(),
            sent: 0,
            cx,
        }
    }
}

impl Write for LiveLog<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.text.extend_from_slice(buf);
        if let Some(end) = buf.iter().rposition(|&b| b == b'\n') {
            let complete = self.text.len() - buf.len() + end + 1;
            if let Some(lines) = self.text.get(self.sent..complete) {
                let lines = String::from_utf8_lossy(lines).into_owned().into_boxed_str();
                self.cx.report(JobProgress::Log(lines));
            }
            self.sent = complete;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn flow(cancel: &Cancel) -> ControlFlow<()> {
    if cancel.is_cancelled() {
        ControlFlow::Break(())
    } else {
        ControlFlow::Continue(())
    }
}

fn apply_into(
    client: &Client,
    schema: &Schema,
    spec: &ApplySpec,
    log: &mut LiveLog<'_>,
) -> Result<Outcome> {
    let input = fs::read_to_string(&spec.path)
        .map_err(|err| io::Error::new(err.kind(), format!("{}: {err}", spec.path.display())))?;
    let plan = Plan::parse(schema, &input)?;
    let mut out = Vec::new();
    let mut reporter = ApplyReporter::new(
        Ansi::new(ColorMode::Disabled),
        Verbosity::Normal,
        OutputFormat::Human,
        Progress::Shown,
        &mut out,
        log,
    );
    reporter.plan_header(plan.stats());
    let on_error = match spec.mode {
        ApplyMode::DryRun => {
            reporter.dry_run_note();
            return Ok(Outcome::Succeeded);
        }
        ApplyMode::Run(on_error) => on_error,
    };
    let summary = plan.apply(client, ApplyOptions { on_error }, &mut |event| {
        reporter.event(event)?;
        flow(&spec.cancel)
    })?;
    Ok(if summary.failed == 0 {
        Outcome::Succeeded
    } else {
        Outcome::Failed
    })
}

fn snapshot_into(
    client: &Client,
    schema: &Schema,
    spec: &SnapshotSpec,
    log: &mut LiveLog<'_>,
) -> Result<Outcome> {
    let snapshot = Snapshot::build(schema, &spec.objects, &[])?;
    let mut reporter =
        SnapshotReporter::new(Ansi::new(ColorMode::Disabled), Verbosity::Normal, log);
    reporter.plan_header(snapshot.stats());
    let target =
        fs::canonicalize(&spec.path).map_or(Cow::Borrowed(spec.path.as_path()), Cow::Owned);
    let existing = fs::metadata(&target).ok().map(|meta| meta.permissions());
    let tmp = tmp_path(&target, process::id(), &AtomicU64::new(0));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if spec.secrets == Secrets::Include {
        OpenOptionsExt::mode(&mut options, PRIVATE_MODE);
    }
    let written = options
        .open(&tmp)
        .map_err(Error::from)
        .and_then(|file| {
            if let Some(permissions) = existing {
                file.set_permissions(permissions)?;
            }
            let mut file = BufWriter::new(file);
            snapshot.write(client, spec.secrets, &mut file, &mut |event| {
                reporter.event(event)?;
                flow(&spec.cancel)
            })?;
            file.flush()?;
            Ok(())
        })
        .and_then(|()| fs::rename(&tmp, &target).map_err(Error::from));
    if let Err(err) = written {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    reporter.done();
    Ok(Outcome::Succeeded)
}

fn report(mut log: Vec<u8>, result: Result<Outcome>) -> ToolReport {
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(err) if is_cancelled(&err) => {
            let _ = writeln!(log, "{err}");
            Outcome::Cancelled
        }
        Err(err) => {
            let _ = writeln!(log, "error: {err}");
            Outcome::Failed
        }
    };
    let log = match String::from_utf8(log) {
        Ok(text) => text,
        Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
    };
    ToolReport {
        log: log.into_boxed_str(),
        outcome,
    }
}

fn is_cancelled(err: &Error) -> bool {
    match err {
        Error::Apply(e) => matches!(**e, ApplyError::Cancelled(_)),
        Error::Snapshot(e) => matches!(**e, SnapshotError::Cancelled),
        _ => false,
    }
}
