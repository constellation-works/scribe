//! `scribe session pass`: write one distilled record per session whose pass
//! is due (docs/design/scribe/2_design.md §5.2, §5.3).
//!
//! The `scribe-session-pass` routine runs it; Daniel can too. It takes the
//! sessions `control.json` marks on and due, at most five per run, oldest
//! id first, and for each:
//!
//! 1. finds its transcript, `<transcripts>/<project-dir>/<id>.jsonl`, and
//!    checks it as a hook does (this user's regular file, named for the
//!    session);
//! 2. reads the whole session ([`pass::conversation`]) and the messages
//!    after the record's `scribed_through`, windowed to 150k characters;
//! 3. asks the `session` model through the `Models` port, within 120 s;
//! 4. checks the answer ([`pass::check`]): a bad budget or shape keeps the
//!    previous record and leaves the pass due; a term whose `said` is not
//!    verbatim from one of Daniel's turns is dropped;
//! 5. writes the record and clears `due` in one store call.
//!
//! Nothing new since the last pass clears `due` without a model call (and
//! marks the record final at session end); no transcript or no message yet
//! leaves the pass due for the next run. After [`pass::MAX_FAILURES`]
//! failures in a row (counted from the audit log) `due` is cleared and a
//! warning logged, so a session that cannot be summarised stops costing a
//! model call every run until the cadence marks it due again.
//!
//! A session never switched on is never read: only sessions with a switch
//! are candidates. A pass writes no entries. Each session passed is one
//! audit line (`target` `session:<id>`); a run with nothing due writes none.

use crate::app::Context;
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
pub(super) const COMMAND: &str = "session pass";

/// The tool sessions are recorded through.
const PROVIDER: &str = "claude-code";
/// The most existing topics, and terms, a request lists.
const MAX_LISTED: usize = 200;

pub(super) const PASS_EXAMPLES: &str = "\
Runs from the scribe-session-pass routine every few minutes. Processes at
most five due sessions per run; --json reports what each pass did and how
many due sessions remain. Calls the configured session model.

Examples:
  scribe session pass
  scribe session pass --json | jq '.passes[] | select(.result == \"failed\")'";

/// Arguments to `session pass`: none.
#[derive(Debug, Args)]
pub(crate) struct PassArgs {}

/// Run the pass with the `claude` CLI as the session model.
pub(super) fn run(ctx: &Context) -> Result<Output, CliError> {
    run_with(ctx, |config| ctx.models(config))
}

/// [`run`] with the `Models` adapter `models` builds for the config, so a
/// test can answer with a fake.
pub(super) fn run_with<M: Models>(
    ctx: &Context,
    models: impl FnOnce(&Config) -> M,
) -> Result<Output, CliError> {
    let store = ctx.store();
    let due = control::due(&store.control()?);
    if due.is_empty() {
        return Ok(Output::Pass(PassReport {
            passes: Vec::new(),
            remaining: 0,
            notices: vec![String::from("no session pass is due")],
        }));
    }
    let config = store.config()?;
    let redactor = ctx.redactor(&config);
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
    };
    let corrupt: Vec<String> = found
        .skipped
        .iter()
        .filter_map(|s| Some(s.path.file_stem()?.to_str()?.to_owned()))
        .collect();
    let adapter = models(&config);
    let remaining = due.len().saturating_sub(pass::MAX_PER_RUN);
    let mut passes = Vec::new();
    for (id, switch) in due.into_iter().take(pass::MAX_PER_RUN) {
        let job = Job {
            ctx,
            config: &config,
            redactor: &redactor,
            id: &id,
            switch: &switch,
        };
        let result = if corrupt.iter().any(|stem| stem == id.as_str()) {
            Err("store_corrupt")
        } else {
            let previous = found.listing.items.iter().find(|r| r.id == id);
            job.run(previous, &known, &adapter)
        };
        passes.push(match result {
            Ok(done) => done,
            Err(code) => job.failed(code),
        });
    }
    Ok(Output::Pass(PassReport {
        passes,
        remaining,
        notices: Vec::new(),
    }))
}

/// The store's topics and terms, offered to the model for reuse.
struct Known {
    topics: Vec<String>,
    terms: Vec<String>,
}

/// One session's pass.
struct Job<'a> {
    ctx: &'a Context,
    config: &'a Config,
    redactor: &'a Redactor,
    id: &'a SessionId,
    switch: &'a SessionControl,
}

/// A failure's stable code.
type Code = &'static str;

impl Job<'_> {
    fn run<M: Models>(
        &self,
        previous: Option<&SessionRecord>,
        known: &Known,
        adapter: &M,
    ) -> Result<PassPayload, Code> {
        let final_pass = self.switch.final_pass;
        let Some(path) = self.ctx.transcripts().and_then(|dir| find(dir, self.id)) else {
            return self.nothing_new(previous, true);
        };
        match payload::check_transcript(&path, self.id, self.ctx.transcripts()) {
            Ok(Transcript::Present) => {}
            Ok(Transcript::Missing) => return self.nothing_new(previous, true),
            Err(reason) => {
                tracing::warn!(session = %self.id, %reason, "not passing: the transcript check failed");
                return Err("provenance_failed");
            }
        }
        let bytes = std::fs::read(&path).map_err(|err| {
            tracing::warn!(session = %self.id, path = %path.display(), %err, "cannot read the transcript");
            "io_error"
        })?;
        let messages = transcript::parse(&bytes);
        let conversation = pass::conversation(&messages);
        if conversation.is_empty() {
            return self.nothing_new(previous, true);
        }
        let window = pass::window(
            &conversation,
            previous.and_then(|p| p.scribed_through.as_deref()),
            pass::WINDOW_CHARS,
        );
        let Some(through) = window.messages.last().map(|m| m.uuid.clone()) else {
            return self.nothing_new(previous, false);
        };
        let request = pass::request(
            &self.config.models.session,
            previous,
            window,
            known.topics.clone(),
            known.terms.clone(),
            final_pass,
        );
        let answer = adapter.session(&request).map_err(|err| {
            tracing::warn!(session = %self.id, %err, "the session model failed");
            err.code()
        })?;
        // One more ask in the same pass, naming what was refused: a model
        // can't count characters exactly, so a near miss is common.
        let distilled = match pass::check(answer, &conversation) {
            Ok(distilled) => distilled,
            Err(err) => {
                tracing::warn!(session = %self.id, %err, "the session model's answer was refused; asking once more");
                let retry = SessionRequest {
                    correction: Some(err.to_string()),
                    ..request
                };
                let answer = adapter.session(&retry).map_err(|err| {
                    tracing::warn!(session = %self.id, %err, "the session model failed");
                    err.code()
                })?;
                pass::check(answer, &conversation).map_err(|err| {
                    tracing::warn!(session = %self.id, %err, "the session model's answer was refused");
                    "session_answer_invalid"
                })?
            }
        };
        let dropped_terms = distilled.dropped_terms;
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
            final_pass,
        };
        let record = pass::record(previous, distilled, meta);
        let written = self.settle(Some(record))?;
        Ok(PassPayload {
            dropped_terms,
            ..self.payload("written", written.as_ref())
        })
    }

    /// Nothing to send the model. At session end the record (if any) is
    /// marked final and the pass settled. Otherwise, when `wait` (no
    /// transcript or message yet) the pass stays due for the next run;
    /// else (nothing new since the last pass) it is settled unchanged.
    fn nothing_new(
        &self,
        previous: Option<&SessionRecord>,
        wait: bool,
    ) -> Result<PassPayload, Code> {
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
    fn settle(&self, record: Option<SessionRecord>) -> Result<Option<SessionRecord>, Code> {
        self.ctx
            .store()
            .settle_pass(self.id, record, self.switch.final_pass, self.redactor)
            .map_err(|err| {
                tracing::warn!(session = %self.id, %err, "cannot write the session pass");
                err.code()
            })
    }

    /// The previous record is kept and the pass stays due, unless this is
    /// the [`pass::MAX_FAILURES`]th failure in a row.
    fn failed(&self, code: Code) -> PassPayload {
        let store = self.ctx.store();
        let before = store
            .failures_in_a_row(COMMAND, &format!("session:{}", self.id))
            .unwrap_or_else(|err| {
                tracing::warn!(session = %self.id, %err, "cannot count earlier failures");
                0
            });
        let gave_up = before.saturating_add(1) >= pass::MAX_FAILURES;
        if gave_up {
            tracing::warn!(
                session = %self.id,
                failures = before.saturating_add(1),
                "session pass failed {} times in a row; clearing due until the cadence marks it again",
                pass::MAX_FAILURES
            );
            if let Err(err) = store.settle_pass(self.id, None, true, self.redactor) {
                tracing::warn!(session = %self.id, %err, "cannot clear due");
            }
        }
        PassPayload {
            session: self.id.to_string(),
            result: "failed",
            error_code: Some(code),
            passes: None,
            dropped_terms: 0,
            gave_up,
        }
    }

    fn payload(&self, result: &'static str, record: Option<&SessionRecord>) -> PassPayload {
        PassPayload {
            session: self.id.to_string(),
            result,
            error_code: None,
            passes: record.map(|r| r.passes),
            dropped_terms: 0,
            gave_up: false,
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
