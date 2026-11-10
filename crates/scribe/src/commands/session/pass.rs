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
//! (120 s), plus one fresh retry for an invalid answer. A session that
//! links to another (`spawned_from`, `continues`) gets that session's
//! latest title and summary as context, and the prompt says to summarize
//! its own slice, not retell the other. Plain headers keep
//! message ids available without XML framing; messages remain data.
//!
//! Every window checks field limits and terms against Daniel's own turns
//! up through that window, then writes a redacted atomic checkpoint under
//! the store lock. Only the last window settles due/final state. Failure
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
use std::path::{Path, PathBuf};

/// The command path in the audit log; also what the failure count reads.
pub(in crate::commands) const COMMAND: &str = "session pass";

/// The tool sessions are recorded through.
const PROVIDER: &str = "claude-code";
/// The most existing topics, and terms, a request lists.
const MAX_LISTED: usize = 200;

pub(super) const PASS_EXAMPLES: &str = "\
The plugin routine ships disabled; once enabled on a host it runs every
5 minutes. Also run this command by hand where claude is on PATH. Takes
at most five due sessions per run, processing every new-message window
(default 40 messages / 15000 characters; [catalog] window_messages and
window_chars). Each window updates and checkpoints the record. --json
reports persisted windows per session and how many due sessions remain.
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
    /// Pass this session now, ignoring retry backoff and whether a pass is due (it must be switched on); repeat for several [default: every due session, at most five]
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
    run_with(
        ctx,
        |config| ctx.models(config),
        RunLimits::new(max),
        &args.sessions,
    )
}

/// [`run`] with the `Models` adapter `models` builds for the config, so a
/// test can answer with a fake, and within `limits`: the plugin's tool
/// starts no session after its start-by time, so a call ends inside
/// Orbit's backend timeout. `only` names the sessions to pass whether or
/// not they are due; empty takes the due ones.
pub(in crate::commands) fn run_with<M: Models>(
    ctx: &Context,
    models: impl FnOnce(&Config) -> M,
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
        }));
    }
    let config = store.config()?;
    let redactor = ctx.redactor(&config)?;
    let found = store.list_sessions(&SessionFilter::default())?;
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
    let mut passes = Vec::new();
    for (n, (id, switch)) in due.into_iter().enumerate() {
        if !limits.may_start(n) {
            remaining = remaining.saturating_add(1);
            continue;
        }
        let mut job = Job {
            ctx,
            config: &config,
            redactor: &redactor,
            id: &id,
            switch: &switch,
            windows: 0,
            dropped_terms: 0,
            incomplete: false,
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
    }))
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
    dropped_terms: usize,
    incomplete: bool,
}

/// A session pass failure, with the stable code kept separate from its
/// redacted, bounded diagnostic.
#[derive(Debug)]
struct PassError {
    code: &'static str,
    message: String,
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
        let final_pass = self.switch.final_pass;
        let Some(path) = self.ctx.transcripts().and_then(|dir| find(dir, self.id)) else {
            return self.nothing_new(previous, true);
        };
        match payload::check_transcript(&path, self.id, self.ctx.transcripts()) {
            Ok(Transcript::Present) => {}
            Ok(Transcript::Missing) => return self.nothing_new(previous, true),
            Err(reason) => {
                tracing::warn!(session = %self.id, %reason, "not passing: the transcript check failed");
                return Err(PassError::new("provenance_failed", reason));
            }
        }
        let bytes = std::fs::read(&path).map_err(|err| {
            tracing::warn!(session = %self.id, path = %path.display(), %err, "cannot read the transcript");
            PassError::new("io_error", format!("cannot read transcript: {err}"))
        })?;
        let messages = transcript::parse(&bytes);
        let conversation = pass::conversation(&messages);
        if conversation.is_empty() {
            return self.nothing_new(previous, true);
        }
        let mut offset = pass::start_after(
            &conversation,
            previous.and_then(|p| p.scribed_through.as_deref()),
        );
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
            let distilled = self.distill(request, &conversation[..end], adapter)?;
            let dropped = distilled.dropped_terms;
            let cwd = transcript::cwd(&bytes).unwrap_or_default();
            let now = self.ctx.now();
            let meta = pass::PassMeta {
                id: self.id.clone(),
                project: if cwd.is_empty() {
                    Project::sanitized("")
                } else {
                    project::of(Path::new(&cwd))
                },
                provider: Label::sanitized(PROVIDER, PROVIDER),
                host: self.ctx.host(),
                cwd,
                started: messages.first().map_or(now, |m| m.at),
                on_since: self.switch.since.unwrap_or(now),
                through: Some(through),
                now,
                final_pass: final_pass && last,
            };
            let record = pass::record(previous.as_ref(), distilled, meta);
            previous = if last {
                self.settle(Some(record))?
            } else {
                self.ctx.store().checkpoint_pass(record, self.redactor).map_err(|err| {
                    tracing::warn!(session = %self.id, %err, "cannot checkpoint the session window");
                    PassError::new(err.code(), err.to_string())
                })?
            };
            self.windows = self.windows.saturating_add(1);
            self.dropped_terms = self.dropped_terms.saturating_add(dropped);
            offset = end;
            if last {
                return Ok(self.payload("written", previous.as_ref()));
            }
        }
    }

    fn distill<M: Models>(
        &self,
        request: SessionRequest,
        conversation: &[scribe_core::models::PassMessage],
        adapter: &M,
    ) -> Result<pass::Distilled, PassError> {
        let answer = adapter.session(&request).map_err(|err| {
            tracing::warn!(session = %self.id, %err, "the session model failed");
            PassError::new(err.code(), err.to_string())
        })?;
        // One more ask in the same pass, naming what was refused: a model
        // can't count characters exactly, so a near miss is common.
        let distilled = match pass::check(answer, conversation) {
            Ok(distilled) => distilled,
            Err(err) => {
                tracing::warn!(session = %self.id, %err, "the session model's answer was refused; asking once more");
                let retry = SessionRequest {
                    correction: Some(err.to_string()),
                    ..request
                };
                let answer = adapter.session(&retry).map_err(|err| {
                    tracing::warn!(session = %self.id, %err, "the session model failed");
                    PassError::new(err.code(), err.to_string())
                })?;
                pass::check(answer, conversation).map_err(|err| {
                    tracing::warn!(session = %self.id, %err, "the session model's answer was refused");
                    PassError::new("session_answer_invalid", err.to_string())
                })?
            }
        };
        Ok(distilled)
    }

    /// Nothing to send the model. At session end the record (if any) is
    /// marked final and the pass settled. Otherwise, when `wait` (no
    /// transcript or message yet) the pass stays due for the next run;
    /// else (nothing new since the last pass) it is settled unchanged.
    fn nothing_new(
        &self,
        previous: Option<&SessionRecord>,
        wait: bool,
    ) -> Result<PassPayload, PassError> {
        if self.switch.final_pass {
            let record = previous.map(|p| pass::finalized(p, self.ctx.now()));
            let written = self.settle(record)?;
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
        self.settle(None)?;
        Ok(self.payload("unchanged", previous))
    }

    /// Write `record` (if any) and settle the switch, in one store call.
    fn settle(&self, record: Option<SessionRecord>) -> Result<Option<SessionRecord>, PassError> {
        self.ctx
            .store()
            .settle_pass(self.id, record, self.switch.final_pass, self.redactor)
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
            gave_up: false,
            error: Some(short_reason(self.redactor, &failure.message)),
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
            gave_up: false,
            error: None,
        }
    }
}

/// The session's transcript: `<dir>/<project-dir>/<id>.jsonl`, the first
/// project directory (by name) that has one. Claude Code names the project
/// directory after the session's cwd, which the store does not keep.
fn find(dir: &Path, id: &SessionId) -> Option<PathBuf> {
    let name = format!("{id}.jsonl");
    let mut projects: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|item| item.file_type().is_ok_and(|t| t.is_dir()))
        .map(|item| item.path())
        .collect();
    projects.sort();
    projects
        .into_iter()
        .map(|project| project.join(&name))
        .find(|path| std::fs::symlink_metadata(path).is_ok())
}
