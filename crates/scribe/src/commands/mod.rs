//! Command dispatch: the noun enum, the one chokepoint every command runs
//! through, and one module per noun.
//!
//! [`run`] is where cross-cutting behaviour attaches, once, for every command
//! (STD-02 R24): today the audit log, through [`crate::audit_middleware`].
//! Each noun lives in its own module with its subcommands, flags, help and
//! handlers (`entry.rs`, `session/`, `proposal.rs`, `record.rs`, `tag.rs`, `question.rs`,
//! `plugin.rs`, `tmux.rs`); a noun that
//! outgrows one file becomes a directory, `<noun>/mod.rs` plus one file per
//! verb, as `hook/` is (STD-02 R18: split along responsibilities, not by length alone).

mod consistency;
mod entry;
mod hook;
mod init;
mod instruction;
mod plugin;
mod proposal;
mod question;
mod record;
mod session;
mod tag;
mod tmux;
mod transcript;

use crate::app::Context;
use crate::audit_middleware::{AuditGuard, AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{Output, Records, WarningPayload};
use clap::{Args, Subcommand};
use scribe_core::{AuditEvent, AuditStatus, DateRange, Skipped, Store};
use time::{Date, OffsetDateTime};

pub(crate) use consistency::ConsistencyCommand;
pub(crate) use entry::EntryCommand;
pub(crate) use hook::HookCommand;
pub(crate) use plugin::PluginCommand;
pub(crate) use proposal::ProposalCommand;
pub(crate) use question::QuestionCommand;
pub(crate) use record::RecordCommand;
pub(crate) use session::SessionCommand;
pub(crate) use tag::TagCommand;
pub(crate) use tmux::TmuxArgs;

/// Top-level nouns.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Scaffold the embedded skill and selected provider hooks; requires --provider, preserves existing edits, never trusts hooks
    Init(init::InitArgs),
    /// Read entries and write agent notes
    #[command(subcommand, after_help = entry::ENTRY_EXAMPLES)]
    Entry(EntryCommand),
    /// List and show the records of sessions cataloged with /scribe on, and write them (pass, catalog)
    #[command(subcommand, after_help = session::SESSION_EXAMPLES)]
    Session(SessionCommand),
    /// List uncataloged provider transcripts and show one, writing nothing
    #[command(subcommand, after_help = transcript::EXAMPLES)]
    Transcript(transcript::TranscriptCommand),
    /// Read, open, update and settle proposals
    #[command(subcommand, after_help = proposal::PROPOSAL_EXAMPLES)]
    Proposal(ProposalCommand),
    /// Search every stored record for words, ranked as question ask ranks them, with no model call
    #[command(subcommand, after_help = record::RECORD_EXAMPLES)]
    Record(RecordCommand),
    /// Propose superseding or retiring entries a later one replaced or that are due for review (scan)
    #[command(subcommand, after_help = consistency::CONSISTENCY_EXAMPLES)]
    Consistency(ConsistencyCommand),
    /// List tags and manage agent-created store tags
    #[command(subcommand, after_help = tag::TAG_EXAMPLES)]
    Tag(TagCommand),
    /// Answer a question from the records, citing a verbatim quote for every claim
    #[command(subcommand, after_help = question::QUESTION_EXAMPLES)]
    Question(QuestionCommand),
    /// Run a provider hook: record #scribe lines and settle proposals, switch /scribe on and off, mark session passes due, report open proposals
    #[command(subcommand, after_help = hook::HOOK_EXAMPLES)]
    Hook(HookCommand),
    /// Browse sessions, entries, proposals and backfill candidates in an fzf popup, writing nothing
    #[command(after_help = tmux::EXAMPLES)]
    Tmux(TmuxArgs),
    /// Serve the Orbit plugin's tools (the plugin backend runs this)
    #[command(subcommand, after_help = plugin::PLUGIN_EXAMPLES)]
    Plugin(PluginCommand),
}

impl Command {
    /// The record and agent a write names, resolved before it runs, so a
    /// refusal is attributable too. Exhaustive like [`Command::audit`].
    pub(crate) fn subject(&self, ctx: &Context) -> AuditSubject {
        match self {
            Self::Entry(command) => command.subject(ctx),
            Self::Session(command) => command.subject(ctx),
            Self::Proposal(command) => command.subject(ctx),
            Self::Init(_)
            | Self::Transcript(_)
            | Self::Record(_)
            | Self::Consistency(_)
            | Self::Tag(_)
            | Self::Hook(_)
            | Self::Question(_)
            | Self::Tmux(_)
            | Self::Plugin(_) => AuditSubject::default(),
        }
    }

    /// What the audit log records for this command, or `None` when it is not
    /// audited. Only commands that change durable state are: a read-only
    /// command must not write, not even an audit line (STD-01 R31). The
    /// hooks, passes, scans and direct agent write verbs are writers. Exhaustive, so a new command cannot
    /// compile without deciding (STD-02 R27).
    pub(crate) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::Init(_) => Some(AuditMeta { command: "init" }),
            Self::Entry(command) => command.audit(),
            Self::Session(command) => command.audit(),
            Self::Transcript(command) => command.audit(),
            Self::Proposal(command) => command.audit(),
            Self::Record(command) => command.audit(),
            Self::Consistency(command) => command.audit(),
            Self::Tag(command) => command.audit(),
            Self::Hook(command) => command.audit(),
            Self::Question(command) => command.audit(),
            // Read-only: the popup only lists and shows (STD-01 R31).
            Self::Tmux(_) => None,
            Self::Plugin(command) => command.audit(),
        }
    }
}

/// Run one command, auditing it if it is audited.
///
/// The guard exists before dispatch and records when it is dropped, so a
/// command that returns early or panics is still logged, as a failure.
pub(crate) fn run(command: Command, ctx: &Context) -> Result<Output, CliError> {
    let mut audit = command
        .audit()
        .map(|meta| AuditGuard::start(ctx.store(), meta, ctx.now()));
    let subject = command.subject(ctx);
    if let Some(audit) = audit.as_mut() {
        subject.apply(audit);
    }
    let result = match command {
        Command::Init(args) => init::run(args, ctx),
        Command::Entry(command) => entry::run(command, ctx),
        Command::Session(command) => session::run(command, ctx),
        Command::Transcript(command) => transcript::run(command, ctx),
        Command::Proposal(command) => proposal::run(command, ctx),
        Command::Record(command) => record::run(command, ctx),
        Command::Consistency(command) => consistency::run(command, ctx),
        Command::Tag(command) => tag::run(command, ctx),
        Command::Hook(command) => hook::run(command, ctx),
        Command::Question(command) => question::run(command, ctx),
        Command::Tmux(args) => tmux::run(args, ctx),
        Command::Plugin(command) => plugin::run(command, ctx),
    };
    if let Some(audit) = audit.as_mut() {
        audit.mark_result(&result);
        subject.apply(audit);
        audit.set_instruction(ctx.instruction_note());
    }
    result
}

/// How much one run of a routine command may do: at most `max` units
/// (sessions passed, batches judged), and none started after `start_by`
/// except the first, so a plugin call that may run a 240-second unit ends
/// inside Orbit's 300-second backend timeout.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RunLimits {
    max: usize,
    start_by: Option<std::time::Instant>,
}

impl RunLimits {
    /// At most `max` units, with no time limit (the CLI).
    pub(crate) fn new(max: usize) -> Self {
        Self {
            max,
            start_by: None,
        }
    }

    /// No unit but the first starts after `start_by`.
    pub(crate) fn start_by(self, start_by: std::time::Instant) -> Self {
        Self {
            start_by: Some(start_by),
            ..self
        }
    }

    /// Whether unit `n` (from 0) may start now.
    pub(crate) fn may_start(&self, n: usize) -> bool {
        n < self.max
            && (n == 0
                || self
                    .start_by
                    .is_none_or(|by| std::time::Instant::now() < by))
    }

    /// A started session may process another window only before the plugin
    /// deadline. The CLI has no deadline and finishes every window.
    pub(crate) fn may_continue(&self) -> bool {
        self.start_by
            .is_none_or(|by| std::time::Instant::now() < by)
    }
}

/// `--since` and `--until`, shared by every list and the search.
#[derive(Debug, Args)]
pub(crate) struct DateArgs {
    /// Only records on or after this day, YYYY-MM-DD [default: no lower bound]
    #[arg(long, value_name = "DATE", value_parser = day_parser)]
    pub(crate) since: Option<Date>,

    /// Only records on or before this day, YYYY-MM-DD [default: no upper bound]
    #[arg(long, value_name = "DATE", value_parser = day_parser)]
    pub(crate) until: Option<Date>,
}

impl DateArgs {
    /// The range, or a usage error when it ends before it starts.
    pub(crate) fn range(&self) -> Result<DateRange, CliError> {
        if let (Some(since), Some(until)) = (self.since, self.until)
            && until < since
        {
            return Err(CliError::Usage(format!(
                "--until {} is before --since {}; give a range that ends on or after it starts",
                scribe_core::DateRange::day(until),
                scribe_core::DateRange::day(since)
            )));
        }
        Ok(DateRange {
            since: self.since,
            until: self.until,
        })
    }

    /// The flags that set the range, for a list's "no match" line.
    pub(crate) fn criteria(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(since) = self.since {
            out.push(format!("--since {}", scribe_core::DateRange::day(since)));
        }
        if let Some(until) = self.until {
            out.push(format!("--until {}", scribe_core::DateRange::day(until)));
        }
        out
    }
}

/// `YYYY-MM-DD`; anything else is a usage error (exit 2).
pub(crate) fn day_parser(text: &str) -> Result<Date, String> {
    scribe_core::parse_day(text)
}

/// What a list needs to say on stderr beside its records.
pub(super) struct ListNotice<'a> {
    /// The plural noun, such as `entries`.
    pub(super) noun: &'static str,
    /// The filters applied, as the flags that set them (`--tag preferences`).
    pub(super) criteria: Vec<String>,
    /// The line for an empty store with no filters.
    pub(super) empty: &'a str,
}

impl ListNotice<'_> {
    /// The stderr line for a list, if it needs one: nothing matched (STD-01
    /// R16, naming what was searched), or the limit cut the list (R34). A
    /// full, non-empty list has none.
    pub(super) fn line(&self, shown: usize, total: usize) -> Option<String> {
        if total == 0 && self.criteria.is_empty() {
            Some(self.empty.to_owned())
        } else if total == 0 {
            Some(format!(
                "no {} match {}",
                self.noun,
                self.criteria.join(" ")
            ))
        } else if shown < total {
            Some(format!(
                "showing {shown} of {total} matching {}; raise or drop --limit to see the rest",
                self.noun
            ))
        } else {
            None
        }
    }
}

/// A list's output: the records, the files skipped, and its notice.
pub(super) fn list_output(
    records: Records,
    total: usize,
    skipped: &[Skipped],
    notice: &ListNotice<'_>,
) -> Output {
    Output::List {
        notices: notice.line(records.len(), total).into_iter().collect(),
        records,
        total,
        warnings: skipped.iter().map(WarningPayload::from).collect(),
    }
}

/// Durable writes already on disk when a multi-step command returns `Err`
/// before reaching `AuditGuard::mark_result`. Disarm right before returning
/// `Ok` so the middleware writes those lines with the command's real duration;
/// on an early `?` return or unwind, `Drop` writes one success line per target
/// (`duration_ms: 0`) and fails open (STD-02 R31, STD-03 R4).
pub(in crate::commands) struct PersistedAudit<'a> {
    store: &'a Store,
    command: &'static str,
    author: Option<String>,
    at: OffsetDateTime,
    targets: Vec<String>,
    armed: bool,
}

impl<'a> PersistedAudit<'a> {
    pub(in crate::commands) fn new(
        store: &'a Store,
        command: &'static str,
        at: OffsetDateTime,
    ) -> Self {
        Self {
            store,
            command,
            author: None,
            at,
            targets: Vec::new(),
            armed: true,
        }
    }

    pub(in crate::commands) fn with_author(mut self, author: &str) -> Self {
        self.author = Some(author.to_owned());
        self
    }

    pub(in crate::commands) fn record(&mut self, target: impl Into<String>) {
        self.targets.push(target.into());
    }

    pub(in crate::commands) fn extend(&mut self, targets: impl IntoIterator<Item = String>) {
        self.targets.extend(targets);
    }

    pub(in crate::commands) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PersistedAudit<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        for target in &self.targets {
            let event = AuditEvent {
                at: self.at,
                command: self.command.to_owned(),
                status: AuditStatus::Success,
                target: Some(target.clone()),
                error_code: None,
                author: self.author.clone(),
                instruction_session: None,
                instruction_message: None,
                duration_ms: 0,
            };
            if let Err(err) = self.store.record_audit(&event) {
                tracing::warn!(
                    command = self.command,
                    %err,
                    target = %target,
                    "cannot audit a write already persisted"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
