//! `scribe session pass`: distill due sessions, checkpointing every window
//! (docs/design/scribe/2_design.md §5.2, §5.3).
//!
//! The plugin ships `scribe-session-pass` seeded disabled for host review.
//! Once enabled it runs every 5 minutes; Daniel can also run the command by
//! hand from a shell where `claude` is on `PATH`. Daniel, 2026-11-10:
//! "perhaps routine was not a bad idea then? let's add that back".
//!
//! At most five on, due sessions run per invocation, by id. Explicit
//! `--session` ids bypass due state, retry backoff and the five-session cap.
//! The transcript is provenance-checked, synthetic turns dropped, and all
//! new messages are processed oldest first in consecutive windows (default
//! 40 messages / 15,000 characters, configured under `[catalog]`). Each
//! window receives the previous record and updates it with one model call
//! (120 s), plus one retry for an invalid answer: header-only compression
//! when only header lengths fail, otherwise a fresh window request. A session
//! that links to another (`spawned_from`, `continues`) gets that session's
//! latest title and summary as context, and the prompt says to summarize
//! its own slice, not retell the other. Plain headers keep
//! message ids available without XML framing; messages remain data.
//!
//! Every window checks field limits and terms against Daniel's own turns
//! up through that window, then writes a redacted atomic checkpoint under
//! the store lock. Before each write the store compares the loaded
//! record's `updated` revision with the record on disk under that lock; a
//! mismatch (for example a rebuild finished while the model ran) refuses
//! the write with `record_changed`, reported with its `error` reason.
//! Only the last window settles due/final state. Failure
//! preserves earlier checkpoints and keeps the session due; automatic
//! retries wait 5, 10, 20, 40, then at most 60 minutes. Success resets the
//! backoff. The CLI finishes all windows; the plugin starts no further
//! window after its deadline, leaving the remainder due for its next run.
//!
//! Nothing new clears due without a model call (and finalizes an ended
//! session); no transcript or messages yet leaves it waiting. Failed runs
//! exit 1 with each failed session, code and short reason; the plugin answers
//! `session_pass_failed`. A never-on session is never read. A pass writes
//! no entries and one audit line per session handled (none for waiting).

use crate::app::Context;
use crate::commands::RunLimits;
use crate::commands::hook::payload::{self, Transcript};
use crate::commands::hook::project;
use crate::error::CliError;
use crate::output::{Output, PassPayload, PassReport};
use clap::Args;
use scribe_core::control::{self};
use scribe_core::models::SessionRequest;
use scribe_core::{
    Config, Label, Models, Project, Redactor, SessionControl, SessionFilter, SessionId,
    SessionRecord, pass, transcript,
};
use std::collections::BTreeSet;
use std::path::Path;

/// The command path in the audit log; also what the failure count reads.
pub(in crate::commands) const COMMAND: &str = "session pass";

/// The most existing topics, and terms, a request lists.
const MAX_LISTED: usize = 200;

pub(super) const PASS_EXAMPLES: &str = "\
The plugin routine ships disabled; once enabled on a host it runs every
5 minutes. Also run this command by hand where each configured provider CLI
is on PATH. Each session uses its stored provider (Codex defaults to Luna). Takes
at most five due sessions per run, processing every new-message window
(default 40 messages / 15000 characters; [catalog] window_messages and
window_chars). Each window rewrites the header and open segment, appends
new segments and checkpoints the record. Sealed segments stay unchanged;
[catalog] segment_windows (default 3) bounds how long a segment stays open.
--json reports windows, segments, seals and how many due sessions remain.
A failed pass stays due with automatic retry delays of 5, 10, 20, 40,
then at most 60 minutes. --session retries immediately. Exits 1 when any
pass failed, naming each failed session, its error code and short reason.

Examples:
  scribe session pass
  scribe session pass --session <id> --session <id>
  scribe session pass --json | jq '.passes[] | select(.result == \"failed\")'";

/// Arguments to `session pass`.
#[derive(Debug, Args)]
pub(crate) struct PassArgs {
    /// Pass this session (its id or a unique prefix of at least four characters) now, ignoring retry backoff and whether a pass is due (it must be switched on); repeat for several [default: every due session, at most five]
    #[arg(long = "session", value_name = "ID")]
    pub(crate) sessions: Vec<SessionId>,
}

/// Run the pass with the `claude` CLI as the session model: at most
/// [`pass::MAX_PER_RUN`] due sessions, or every session `args` names.
pub(super) fn run(args: &PassArgs, ctx: &Context) -> Result<Output, CliError> {
    let max = if args.sessions.is_empty() {
        pass::MAX_PER_RUN
    } else {
        args.sessions.len()
    };
    // A prefix names its session; an id nothing matches is kept, so the
    // pass refuses it as not switched on.
    let sessions = args
        .sessions
        .iter()
        .map(|id| match ctx.store().resolve_session(id) {
            Err(scribe_core::Error::SessionNotFound { .. }) => Ok(id.clone()),
            other => other,
        })
        .collect::<Result<Vec<_>, _>>()?;
    run_with(
        ctx,
        |config| ctx.models(config),
        RunLimits::new(max),
        &sessions,
    )
}

/// [`run`] with the `Models` adapter `models` builds for the config, so a
/// test can answer with a fake, and within `limits`: the plugin's tool
/// starts no session after its start-by time, so a call ends inside
/// Orbit's backend timeout. `only` names the sessions to pass whether or
/// not they are due; empty takes the due ones.
pub(in crate::commands) fn run_with<M: Models>(
    ctx: &Context,
    mut models: impl FnMut(&Config) -> M,
    limits: RunLimits,
    only: &[SessionId],
) -> Result<Output, CliError> {
    let store = ctx.store();
    let state = store.control()?;
    let due = if only.is_empty() {
        control::due(&state)
    } else {
        control::chosen(&state, only).map_err(CliError::SessionNotOn)?
    };
    let mut remaining = 0usize;
    let due: Vec<_> = due
        .into_iter()
        .filter(|(_, c)| {
            let ready = !only.is_empty() || c.retry_after.is_none_or(|at| ctx.now() >= at);
            if !ready {
                remaining = remaining.saturating_add(1);
            }
            ready
        })
        .collect();
    if due.is_empty() {
        return Ok(Output::Pass(PassReport {
            passes: Vec::new(),
            remaining,
            notices: vec![if remaining == 0 {
                String::from("no session pass is due")
            } else {
                format!("{remaining} due session pass(es) waiting for retry backoff")
            }],
            catalog: false,
            switch_written: false,
        }));
    }
    let config = store.config()?;
    let redactor = ctx.redactor(&config)?;
    let found = store.list_sessions(&SessionFilter {
        include_inactive: true,
        ..Default::default()
    })?;
    let mut topics: BTreeSet<String> = BTreeSet::new();
    let mut terms: BTreeSet<String> = BTreeSet::new();
    for record in &found.listing.items {
        topics.extend(record.topics.as_slice().iter().map(ToString::to_string));
        terms.extend(record.terms.iter().map(|t| t.term.clone()));
    }
    let known = Known {
        topics: topics.into_iter().take(MAX_LISTED).collect(),
        terms: terms.into_iter().take(MAX_LISTED).collect(),
        records: &found.listing.items,
    };
    let corrupt: Vec<(String, String)> = found
        .skipped
        .iter()
        .filter_map(|s| {
            Some((
                s.path.file_stem()?.to_str()?.to_owned(),
                s.error.to_string(),
            ))
        })
        .collect();
    let mut passes = Vec::new();
    for (n, (id, switch)) in due.into_iter().enumerate() {
        if !limits.may_start(n) {
            remaining = remaining.saturating_add(1);
            continue;
        }
        let scoped = ctx.with_provider(switch.provider);
        let config = config.for_provider(switch.provider);
        let adapter = models(&config);
        let mut job = Job {
            ctx: &scoped,
            config: &config,
            redactor: &redactor,
            id: &id,
            switch: &switch,
            windows: 0,
            segments_sealed: 0,
            segments: None,
            dropped_terms: 0,
            warnings: Vec::new(),
            incomplete: false,
            catalog: None,
        };
        let result = if let Some((_, reason)) = corrupt.iter().find(|(stem, _)| stem == id.as_str())
        {
            Err(PassError::new("store_corrupt", reason.clone()))
        } else {
            let previous = found.listing.items.iter().find(|r| r.id == id);
            job.run(previous, &known, &adapter, &limits)
        };
        passes.push(match result {
            Ok(done) => done,
            Err(failure) => job.failed(failure)?,
        });
        if job.incomplete {
            remaining = remaining.saturating_add(1);
        }
    }
    Ok(Output::Pass(PassReport {
        passes,
        remaining,
        notices: Vec::new(),
        catalog: false,
        switch_written: false,
    }))
}

/// Run one `session catalog` after its switch is written. No deadline and
/// no five-session cap. `project` is used only when the session has no
/// record yet. `ended` marks the last window final and turns the switch off.
pub(in crate::commands) fn run_catalog<M: Models>(
    ctx: &Context,
    models: impl FnOnce(&Config) -> M,
    id: &SessionId,
    switch: &SessionControl,
    project: Project,
    ended: bool,
) -> Result<PassReport, CliError> {
    let store = ctx.store();
    let config = ctx.config()?;
    let redactor = ctx.redactor(&config)?;
    let found = store.list_sessions(&SessionFilter {
        include_inactive: true,
        ..Default::default()
    })?;
    let mut topics: BTreeSet<String> = BTreeSet::new();
    let mut terms: BTreeSet<String> = BTreeSet::new();
    for record in &found.listing.items {
        topics.extend(record.topics.as_slice().iter().map(ToString::to_string));
        terms.extend(record.terms.iter().map(|t| t.term.clone()));
    }
    let known = Known {
        topics: topics.into_iter().take(MAX_LISTED).collect(),
        terms: terms.into_iter().take(MAX_LISTED).collect(),
        records: &found.listing.items,
    };
    let corrupt: Vec<(String, String)> = found
        .skipped
        .iter()
        .filter_map(|s| {
            Some((
                s.path.file_stem()?.to_str()?.to_owned(),
                s.error.to_string(),
            ))
        })
        .collect();
    let adapter = models(&config);
    let mut job = Job {
        ctx,
        config: &config,
        redactor: &redactor,
        id,
        switch,
        windows: 0,
        segments_sealed: 0,
        segments: None,
        dropped_terms: 0,
        warnings: Vec::new(),
        incomplete: false,
        catalog: Some(CatalogRun { project, ended }),
    };
    let limits = RunLimits::new(1);
    let result = if let Some((_, reason)) = corrupt.iter().find(|(stem, _)| stem == id.as_str()) {
        Err(PassError::new("store_corrupt", reason.clone()))
    } else {
        let previous = found.listing.items.iter().find(|r| &r.id == id);
        job.run(previous, &known, &adapter, &limits)
    };
    let passes = vec![match result {
        Ok(done) => done,
        Err(failure) => job.failed(failure)?,
    }];
    Ok(PassReport {
        passes,
        remaining: 0,
        notices: Vec::new(),
        catalog: true,
        switch_written: false,
    })
}

/// The store's topics and terms, offered to the model for reuse, and its
/// records, for a linked session's context.
struct Known<'a> {
    topics: Vec<String>,
    terms: Vec<String>,
    records: &'a [SessionRecord],
}

/// One session's pass.
struct Job<'a> {
    ctx: &'a Context,
    config: &'a Config,
    redactor: &'a Redactor,
    id: &'a SessionId,
    switch: &'a SessionControl,
    windows: usize,
    segments_sealed: usize,
    segments: Option<usize>,
    dropped_terms: usize,
    warnings: Vec<String>,
    incomplete: bool,
    /// Set for `session catalog`: the project of the first record, and
    /// whether the transcript's last message is at least a day old.
    catalog: Option<CatalogRun>,
}

/// One cataloged session's extra facts. Absent for an ordinary pass.
struct CatalogRun {
    project: Project,
    ended: bool,
}

/// A session pass failure, with the stable code kept separate from its
/// redacted, bounded diagnostic.
#[derive(Debug)]
pub(super) struct PassError {
    pub(super) code: &'static str,
    pub(super) message: String,
}

impl PassError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            code,
            message: if message.trim().is_empty() {
                code.to_owned()
            } else {
                message
            },
        }
    }
}

/// The maximum characters of a reason exposed in a pass report.
const MAX_ERROR_CHARS: usize = 300;

/// Redact, flatten and bound a failure reason before it reaches a report or
/// the aggregate command error.
pub(super) fn short_reason(redactor: &Redactor, message: &str) -> String {
    let flattened = redactor
        .mask(message)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = flattened.chars();
    let mut bounded: String = chars.by_ref().take(MAX_ERROR_CHARS).collect();
    if chars.next().is_some() {
        bounded = bounded.chars().take(MAX_ERROR_CHARS - 1).collect();
        bounded.push('…');
    }
    bounded
}

impl Job<'_> {
    fn run<M: Models>(
        &mut self,
        previous: Option<&SessionRecord>,
        known: &Known<'_>,
        adapter: &M,
        limits: &RunLimits,
    ) -> Result<PassPayload, PassError> {
        self.segments = previous.map(|p| p.log.len());
        let Some(path) = payload::locate(self.ctx, self.id)
            .map_err(|e| PassError::new(e.code(), e.to_string()))?
        else {
            return self.nothing_new(previous, true);
        };
        match payload::check(self.ctx, &path, self.id) {
            Ok(Transcript::Present) => {}
            Ok(Transcript::Missing) => return self.nothing_new(previous, true),
            Err(reason) => {
                tracing::warn!(session = %self.id, %reason, "not passing: the transcript check failed");
                return Err(PassError::new("provenance_failed", reason));
            }
        }
        let bytes = payload::read_for(self.ctx, &path).map_err(|err| {
            tracing::warn!(session = %self.id, path = %path.display(), %err, "cannot read the transcript");
            PassError::new("io_error", format!("cannot read transcript: {err}"))
        })?;
        // A catalog switch that `session catalog` did not run itself (one
        // `/scribe catalog <id>` wrote, or a failed catalog left due) is
        // passed as a catalog: its windows are `origin: catalog`, it keeps
        // `cataloged_by`, and an ended transcript ends final and off.
        if self.catalog.is_none() && self.switch.cataloged_by.is_some() {
            self.catalog = Some(CatalogRun {
                project: super::super::transcript::attributed_project(&bytes, self.config),
                ended: transcript::survey(&bytes)
                    .ended
                    .is_some_and(|last| transcript::is_day_old(last, self.ctx.now())),
            });
        }
        let final_pass = match &self.catalog {
            Some(catalog) => catalog.ended,
            None => self.switch.final_pass,
        };
        let messages = transcript::parse(&bytes);
        let conversation = pass::conversation(&messages);
        if conversation.is_empty() {
            return self.nothing_new(previous, true);
        }
        let mut offset = pass::start_after(
            &conversation,
            previous.and_then(|p| p.scribed_through.as_deref()),
        );
        let mut expected = previous.map(|p| p.updated);
        let mut previous = previous.cloned();
        // The linked sessions' latest records, read once: context only.
        let mut links = previous
            .as_ref()
            .map(|p| p.links.clone())
            .unwrap_or_default();
        for link in &self.switch.pending_links {
            if !links.contains(link) {
                links.push(link.clone());
            }
        }
        let linked = pass::linked_context(&links, known.records);
        loop {
            let window =
                pass::window_with_limits(&conversation[offset..], None, self.config.catalog_window);
            let Some(through) = window.messages.last().map(|m| m.uuid.clone()) else {
                return self.nothing_new(previous.as_ref(), false);
            };
            let first_message = window.messages.first().map(|m| m.uuid.clone());
            let window_meta = window
                .messages
                .iter()
                .map(|m| {
                    (
                        m.uuid.clone(),
                        messages
                            .iter()
                            .find(|sent| sent.uuid == m.uuid)
                            .map_or(self.ctx.now(), |sent| sent.at),
                    )
                })
                .collect();
            if self.windows > 0 && !limits.may_continue() {
                self.incomplete = true;
                return Ok(self.payload("written", previous.as_ref()));
            }
            // Citation validation includes older windows, never future messages.
            let end = offset + window.messages.len();
            let last = end == conversation.len();
            let mut topics: BTreeSet<_> = known.topics.iter().cloned().collect();
            let mut terms: BTreeSet<_> = known.terms.iter().cloned().collect();
            if let Some(record) = &previous {
                topics.extend(record.topics.as_slice().iter().map(ToString::to_string));
                terms.extend(record.terms.iter().map(|t| t.term.clone()));
            }
            let request = pass::request(
                &self.config.models.session,
                previous.as_ref(),
                linked.clone(),
                window,
                topics.into_iter().take(MAX_LISTED).collect(),
                terms.into_iter().take(MAX_LISTED).collect(),
                final_pass && last,
            );
            let distilled = distill(self.id, request, &conversation[..end], adapter)?;
            let dropped = distilled.dropped_terms;
            let warnings = window_warnings(self.windows + 1, &distilled);
            let cwd = transcript::cwd(&bytes).unwrap_or_default();
            let now = self.ctx.now();
            // The window's first and last messages' times bound the segment.
            let sent = |uuid: Option<&str>| {
                uuid.and_then(|u| messages.iter().find(|m| m.uuid == u))
                    .map_or(now, |m| m.at)
            };
            let from = sent(first_message.as_deref());
            let to = sent(Some(&through));
            let mut project = if cwd.is_empty() {
                Project::sanitized("")
            } else {
                project::of(Path::new(&cwd))
            };
            // `--project`, or the transcript-list attribution, only for the
            // first record. A resumed catalog keeps the record's project.
            if previous.is_none()
                && let Some(catalog) = &self.catalog
            {
                project = catalog.project.clone();
            }
            let meta = pass::PassMeta {
                id: self.id.clone(),
                project,
                provider: Label::sanitized(
                    self.ctx.provider().as_str(),
                    self.ctx.provider().as_str(),
                ),
                host: self.ctx.host(),
                cwd,
                started: messages.first().map_or(now, |m| m.at),
                on_since: self.switch.since.unwrap_or(now),
                through: Some(through),
                first_message,
                from,
                to,
                now,
                final_pass: final_pass && last,
                window: window_meta,
                segment_windows: self.config.segment_windows,
                origin: if self.catalog.is_some() {
                    scribe_core::SegmentOrigin::Catalog
                } else {
                    scribe_core::SegmentOrigin::Pass
                },
            };
            let sealed_before = previous.as_ref().map_or(0, |p| {
                p.log
                    .iter()
                    .filter(|s| s.state == scribe_core::SegmentState::Sealed)
                    .count()
            });
            let record = self.stamp_catalog(pass::record(previous.as_ref(), distilled, meta));
            let sealed_after = record
                .log
                .iter()
                .filter(|s| s.state == scribe_core::SegmentState::Sealed)
                .count();
            previous = if !last {
                self.checkpoint(record, expected)?
            } else if self.catalog.as_ref().is_some_and(|c| c.ended) {
                self.settle_off(Some(record), expected)?
            } else if self.catalog.is_some() {
                self.checkpoint_live(record, expected)?
            } else {
                self.settle(Some(record), expected)?
            };
            expected = previous.as_ref().map(|p| p.updated);
            self.segments_sealed += sealed_after.saturating_sub(sealed_before);
            self.segments = previous.as_ref().map(|r| r.log.len());
            self.windows = self.windows.saturating_add(1);
            self.dropped_terms = self.dropped_terms.saturating_add(dropped);
            self.warnings.extend(warnings);
            offset = end;
            if last {
                return Ok(self.payload("written", previous.as_ref()));
            }
        }
    }

    /// Nothing to send the model. At session end the record (if any) is
    /// marked final and the pass settled. Otherwise, when `wait` (no
    /// transcript or message yet) the pass stays due for the next run;
    /// else (nothing new since the last pass) it is settled unchanged.
    fn nothing_new(
        &mut self,
        previous: Option<&SessionRecord>,
        wait: bool,
    ) -> Result<PassPayload, PassError> {
        if self.catalog.is_some() {
            let ended = self.catalog.as_ref().is_some_and(|c| c.ended);
            // An ended transcript with nothing left to send is still final,
            // and the switch turns off with that write. A live one, or a
            // transcript with no message yet, stays on and due.
            if ended && !wait {
                let record = previous.map(|p| {
                    let mut finalized = pass::finalized(p, self.ctx.now());
                    if finalized.cataloged_by.is_none() {
                        finalized.cataloged_by.clone_from(&self.switch.cataloged_by);
                    }
                    finalized
                });
                let newly_sealed = previous.is_some_and(|p| p.open_segment().is_some());
                let written = self.settle_off(record, previous.map(|p| p.updated))?;
                self.segments_sealed += usize::from(newly_sealed);
                let result = if written.is_some() {
                    "written"
                } else {
                    "unchanged"
                };
                return Ok(self.payload(result, written.as_ref()));
            }
            let result = if wait { "waiting" } else { "unchanged" };
            return Ok(self.payload(result, previous));
        }
        if self.switch.final_pass {
            let record = previous.map(|p| pass::finalized(p, self.ctx.now()));
            let newly_sealed = previous.is_some_and(|p| p.open_segment().is_some());
            let written = self.settle(record, previous.map(|p| p.updated))?;
            self.segments_sealed += usize::from(newly_sealed);
            let result = if written.is_some() {
                "written"
            } else {
                "unchanged"
            };
            return Ok(self.payload(result, written.as_ref()));
        }
        if wait {
            return Ok(self.payload("waiting", previous));
        }
        self.settle(None, previous.map(|p| p.updated))?;
        Ok(self.payload("unchanged", previous))
    }

    /// Copy `cataloged_by` from the switch onto the first record. Later
    /// windows keep it through [`pass::record`].
    fn stamp_catalog(&self, mut record: SessionRecord) -> SessionRecord {
        if self.catalog.is_some() && record.cataloged_by.is_none() {
            record.cataloged_by.clone_from(&self.switch.cataloged_by);
        }
        record
    }

    fn checkpoint(
        &self,
        record: SessionRecord,
        expected: Option<time::OffsetDateTime>,
    ) -> Result<Option<SessionRecord>, PassError> {
        self.ctx
            .store()
            .checkpoint_pass(record, expected, self.redactor)
            .map_err(|err| {
                tracing::warn!(session = %self.id, %err, "cannot checkpoint the session window");
                PassError::new(err.code(), err.to_string())
            })
    }

    /// A live catalog's last window: the switch stays on and due.
    fn checkpoint_live(
        &self,
        record: SessionRecord,
        expected: Option<time::OffsetDateTime>,
    ) -> Result<Option<SessionRecord>, PassError> {
        self.ctx
            .store()
            .checkpoint_catalog(record, expected, self.redactor)
            .map_err(|err| {
                tracing::warn!(session = %self.id, %err, "cannot checkpoint the catalog window");
                PassError::new(err.code(), err.to_string())
            })
    }

    /// An ended catalog's last window: final record and the switch off,
    /// in one store call.
    fn settle_off(
        &self,
        record: Option<SessionRecord>,
        expected: Option<time::OffsetDateTime>,
    ) -> Result<Option<SessionRecord>, PassError> {
        self.ctx
            .store()
            .settle_catalog_final(self.id, record, expected, self.redactor)
            .map_err(|err| {
                tracing::warn!(session = %self.id, %err, "cannot finish the catalog");
                PassError::new(err.code(), err.to_string())
            })
    }

    /// Write `record` (if any) and settle the switch, in one store call.
    /// `expected` is the loaded record's `updated` revision, checked by the
    /// store under its lock; a refusal keeps the session due through
    /// [`Job::failed`].
    fn settle(
        &self,
        record: Option<SessionRecord>,
        expected: Option<time::OffsetDateTime>,
    ) -> Result<Option<SessionRecord>, PassError> {
        self.ctx
            .store()
            .settle_pass(
                self.id,
                record,
                self.switch.final_pass,
                expected,
                self.redactor,
            )
            .map_err(|err| {
                tracing::warn!(session = %self.id, %err, "cannot write the session pass");
                PassError::new(err.code(), err.to_string())
            })
    }

    /// The previous record is kept and the pass stays due, so the next run
    /// retries it.
    fn failed(&self, failure: PassError) -> Result<PassPayload, CliError> {
        self.ctx
            .store()
            .update_control(|state| control::fail(state, self.id, self.ctx.now()))?;
        Ok(PassPayload {
            session: self.id.to_string(),
            result: "failed",
            error_code: Some(failure.code),
            passes: None,
            dropped_terms: self.dropped_terms,
            windows: self.windows,
            segments_sealed: self.segments_sealed,
            segments: self.segments,
            author: None,
            gave_up: false,
            error: Some(short_reason(self.redactor, &failure.message)),
            warnings: self.warnings.clone(),
        })
    }

    fn payload(&self, result: &'static str, record: Option<&SessionRecord>) -> PassPayload {
        PassPayload {
            session: self.id.to_string(),
            result,
            error_code: None,
            passes: record.map(|r| r.passes),
            dropped_terms: self.dropped_terms,
            windows: self.windows,
            segments_sealed: self.segments_sealed,
            segments: record.map(|r| r.log.len()),
            author: None,
            gave_up: false,
            error: None,
            warnings: self.warnings.clone(),
        }
    }
}

/// The session's transcript: `<dir>/<project-dir>/<id>.jsonl`, the first
/// project directory (by name) that has one. Claude Code names the project
/// directory after the session's cwd, which the store does not keep.
pub(super) fn distill<M: Models>(
    id: &SessionId,
    request: SessionRequest,
    conversation: &[scribe_core::models::PassMessage],
    adapter: &M,
) -> Result<pass::Distilled, PassError> {
    let mut answer = adapter.session(&request).map_err(|err| {
        tracing::warn!(session = %id, %err, "the session model failed");
        PassError::new(err.code(), err.to_string())
    })?;
    // One more ask in the same pass, naming what was refused: a model
    // can't count characters exactly, so a near miss is common. A refusal
    // the retry or the repair resolves is logged at info; only a pass that
    // still fails warns, naming both refusals. Repairs reach the report.
    let distilled = match pass::check(answer.clone(), conversation, &request) {
        Ok(distilled) => distilled,
        Err(err) => {
            tracing::info!(session = %id, %err, "the session model's answer was refused; asking once more");
            let refusal = err.to_string();
            let retry_failure = |err: scribe_core::models::ModelError| {
                tracing::warn!(session = %id, %refusal, %err, "the session model failed on the retry");
                PassError::new(err.code(), format!("{refusal}; retry: {err}"))
            };
            if let Some(compress) = pass::compression_request(&answer, conversation, &request) {
                let header = adapter.session_header(&compress).map_err(retry_failure)?;
                answer.title = header.title;
                answer.summary = header.summary;
                answer.goals = header.goals;
                answer.next = header.next;
            } else {
                let retry = SessionRequest {
                    correction: Some(refusal.clone()),
                    ..request.clone()
                };
                answer = adapter.session(&retry).map_err(retry_failure)?;
            }
            pass::check(answer.clone(), conversation, &request)
                .or_else(|_| pass::repair(answer, conversation, &request))
                .map_err(|err| {
                    tracing::warn!(session = %id, %refusal, %err, "the session model's answer was refused twice");
                    PassError::new("session_answer_invalid", format!("{refusal}; retry: {err}"))
                })?
        }
    };
    Ok(distilled)
}

/// Repairs describe only fixed rule names and numbers; no model text or ids.
pub(super) fn window_warnings(window: usize, answer: &pass::Distilled) -> Vec<String> {
    answer
        .repairs
        .iter()
        .map(|repair| format!("window {window}: {repair}"))
        .collect()
}
