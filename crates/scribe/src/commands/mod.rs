//! Command dispatch: the noun enum, the one chokepoint every command runs
//! through, and one module per noun.
//!
//! [`run`] is where cross-cutting behaviour attaches, once, for every command
//! (STD-02 R24): today the audit log, through [`crate::audit_middleware`].
//! Each noun lives in its own module with its subcommands, flags, help and
//! handlers (`entry.rs`, `session/`, `proposal.rs`, `record.rs`, `tag.rs`, `question.rs`,
//! `brief.rs`, `store.rs`, `export.rs`, `plugin.rs`, `tmux.rs`); a noun that
//! outgrows one file becomes a directory, `<noun>/mod.rs` plus one file per
//! verb, as `hook/` is (STD-02 R18: split along responsibilities, not by length alone).

pub(in crate::commands) mod brief;
mod consistency;
mod entry;
mod export;
mod group;
mod hook;
mod init;
mod instruction;
pub(in crate::commands) mod memory;
mod plugin;
mod proposal;
mod question;
mod search;
mod session;
mod store;
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
pub(crate) use memory::MemoryCommand;
pub(crate) use plugin::PluginCommand;
pub(crate) use proposal::ProposalCommand;
pub(crate) use question::QuestionCommand;
pub(crate) use session::SessionCommand;
pub(crate) use store::StoreCommand;
pub(crate) use tag::TagCommand;
pub(crate) use tmux::TmuxArgs;

/// Top-level nouns.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Scaffold the embedded skill and selected provider hooks
    ///
    /// Requires --provider, preserves existing edits, and never trusts hooks.
    Init(init::InitArgs),
    /// Read entries and write agent notes
    #[command(subcommand, after_help = entry::ENTRY_EXAMPLES)]
    Entry(EntryCommand),
    /// Group sessions into named seats for a shared piece of work
    #[command(subcommand, after_help = group::EXAMPLES)]
    Group(group::GroupCommand),
    /// List, show and write session records cataloged with /scribe on
    ///
    /// Lists and shows cataloged session records, and writes them with pass and catalog.
    #[command(subcommand, after_help = session::SESSION_EXAMPLES)]
    Session(SessionCommand),
    /// List uncataloged provider transcripts and show one, writing nothing
    #[command(subcommand, after_help = transcript::EXAMPLES)]
    Transcript(transcript::TranscriptCommand),
    /// Read, open, update and settle proposals
    #[command(subcommand, after_help = proposal::PROPOSAL_EXAMPLES)]
    Proposal(ProposalCommand),
    /// Show what scribe noticed about the user and manage memory items
    ///
    /// Shows what scribe has noticed about the user from switched-on sessions, and lets you acknowledge, retire or forget an item.
    #[command(subcommand, after_help = memory::MEMORY_EXAMPLES)]
    Memory(MemoryCommand),
    /// Search every stored record by words, with no model call
    ///
    /// Ranks entries, session headers and segments for words, best first, as question ask ranks them, with no model call.
    #[command(after_help = search::SEARCH_EXAMPLES)]
    Search(search::SearchArgs),
    /// Answer a question from stored records with verbatim quotes
    #[command(after_help = question::ASK_EXAMPLES)]
    Ask(question::AskArgs),
    /// Propose superseding or retiring entries via scan
    ///
    /// Proposes superseding or retiring entries a later one replaced or that are due for review, and deactivates stale or removed-feature sessions via scan.
    #[command(subcommand, after_help = consistency::CONSISTENCY_EXAMPLES)]
    Consistency(ConsistencyCommand),
    /// Move the store's session files into scribe.db
    ///
    /// Moves the session files into scribe.db now; every write does it on its own, keeping the old files aside.
    #[command(subcommand, after_help = store::EXAMPLES)]
    Store(StoreCommand),
    /// Write every session to a directory as Markdown or JSONL
    ///
    /// Writes every session to a directory you name, as Markdown in the old record files' shape or as JSONL, one line per row; the backup and the way out, writing nothing to the store.
    #[command(after_help = export::EXAMPLES)]
    Export(export::ExportArgs),
    /// List tags and manage agent-created store tags
    #[command(subcommand, after_help = tag::TAG_EXAMPLES)]
    Tag(TagCommand),
    /// Show the session-start page: memory, recent sessions, open claims
    ///
    /// Shows the session-start page the hook injects: the memory page, the last sessions (this project's first) and every open claim grouped by tag with the id that settles it; model-free, writing nothing.
    #[command(after_help = brief::EXAMPLES)]
    Brief(brief::BriefArgs),
    /// Answer a question from the records with verbatim quotes
    ///
    /// Answers a question from the records, citing a verbatim quote for every claim.
    #[command(subcommand, after_help = question::QUESTION_EXAMPLES)]
    Question(QuestionCommand),
    /// Run a provider hook for session and prompt events
    ///
    /// Runs a provider hook to record #scribe lines and settle proposals, switch /scribe on and off, mark session passes due, and report open proposals.
    #[command(subcommand, after_help = hook::HOOK_EXAMPLES)]
    Hook(HookCommand),
    /// Browse sessions, entries and proposals in an fzf popup
    ///
    /// Browses sessions, entries, proposals and backfill candidates in an fzf popup, writing nothing.
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
            Self::Group(command) => command.subject(ctx),
            Self::Entry(command) => command.subject(ctx),
            Self::Session(command) => command.subject(ctx),
            Self::Proposal(command) => command.subject(ctx),
            Self::Memory(command) => command.subject(ctx),
            Self::Init(_)
            | Self::Transcript(_)
            | Self::Search(_)
            | Self::Ask(_)
            | Self::Consistency(_)
            | Self::Tag(_)
            | Self::Hook(_)
            | Self::Question(_)
            | Self::Brief(_)
            | Self::Store(_)
            | Self::Export(_)
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
            Self::Init(_) => Some(AuditMeta {
                command: "init",
                stepwise: false,
            }),
            Self::Group(command) => command.audit(),
            Self::Entry(command) => command.audit(),
            Self::Session(command) => command.audit(),
            Self::Transcript(command) => command.audit(),
            Self::Proposal(command) => command.audit(),
            Self::Memory(command) => command.audit(),
            Self::Search(args) => search::audit(args),
            Self::Consistency(command) => command.audit(),
            Self::Store(command) => command.audit(),
            Self::Tag(command) => command.audit(),
            Self::Hook(command) => command.audit(),
            Self::Question(command) => command.audit(),
            // Read-only: the popup only lists and shows, the brief only
            // reads, and an export writes nothing to the store (STD-01 R31).
            Self::Tmux(_) | Self::Brief(_) | Self::Export(_) | Self::Ask(_) => None,
            Self::Plugin(command) => command.audit(),
        }
    }
}

/// The short forms of the session ids `output` prints in a terminal table:
/// unique across every session the store knows, not only the rows shown, so
/// what is printed is accepted back (STD-01 R32). A `transcript list` also
/// counts the transcripts on disk, which `transcript show` resolves against.
/// Nothing is computed for an output that prints no session id; a read that
/// fails leaves the table with full ids rather than failing the command
/// (STD-02 R31).
pub(crate) fn short_ids(output: &Output, ctx: &Context) -> scribe_core::ShortIds {
    use crate::output::Record;
    let prints_sessions = match output {
        Output::List { records, .. } => matches!(
            records,
            Records::Sessions { .. } | Records::SessionTree { .. } | Records::Transcripts(_)
        ),
        Output::Pass(_) => true,
        Output::Show { record } | Output::Written { record, .. } => {
            matches!(record, Record::Group(_))
        }
        Output::Init(_)
        | Output::Hook(_)
        | Output::Scan(_)
        | Output::Ask(_)
        | Output::Plugin(_)
        | Output::Popup
        | Output::PopupRows(_)
        | Output::PopupPreview(_) => false,
    };
    if !prints_sessions {
        return scribe_core::ShortIds::default();
    }
    let mut ids = ctx.store().session_ids().unwrap_or_default();
    if matches!(
        output,
        Output::List {
            records: Records::Transcripts(_),
            ..
        }
    ) && let Ok(extra) = transcript_session_ids(ctx)
    {
        ids.extend(extra);
    }
    scribe_core::ShortIds::new(ids)
}

/// The session ids of provider transcripts on disk, for resolving prefixes
/// of uncataloged sessions alongside the store's own ids.
pub(in crate::commands) fn transcript_session_ids(ctx: &Context) -> Result<Vec<String>, CliError> {
    Ok(hook::payload::files_for(ctx)?
        .into_iter()
        .map(|(id, _)| id.to_string())
        .collect())
}

/// Resolve a session id or unique prefix across the store and provider
/// transcripts on disk, returning an unknown id as typed so its own check can
/// report it. Ambiguity and short prefixes are still refused.
pub(in crate::commands) fn resolve_session_or_typed(
    ctx: &Context,
    input: &scribe_core::SessionId,
) -> Result<scribe_core::SessionId, CliError> {
    let extra = transcript_session_ids(ctx)?;
    Ok(ctx.store().resolve_session_or_typed_with(input, &extra)?)
}

/// Resolve one `--related` or `--result` `session:<id>` reference across the
/// store and provider transcripts on disk, leaving other kinds and unknown
/// ids as typed.
pub(in crate::commands) fn resolve_related(
    ctx: &Context,
    related: &scribe_core::Related,
) -> Result<scribe_core::Related, CliError> {
    if related.kind() != scribe_core::RelatedKind::Session {
        return Ok(related.clone());
    }
    let extra = transcript_session_ids(ctx)?;
    Ok(ctx.store().resolve_related_with(related, &extra)?)
}

/// Resolve `session:<id>` references in `related` across the store and
/// provider transcripts on disk, scanning transcripts at most once.
pub(in crate::commands) fn resolve_relateds(
    ctx: &Context,
    related: &[scribe_core::Related],
) -> Result<Vec<scribe_core::Related>, CliError> {
    if !related
        .iter()
        .any(|r| r.kind() == scribe_core::RelatedKind::Session)
    {
        return Ok(related.to_vec());
    }
    let extra = transcript_session_ids(ctx)?;
    Ok(ctx.store().resolve_relateds_with(related, &extra)?)
}

/// Run one command, auditing it if it is audited.
///
/// The guard exists before dispatch and records when it is dropped, so a
/// command that returns early or panics is still logged, as a failure. An
/// audited command that calls no model runs as one store transaction with
/// its audit lines ([`AuditMeta::stepwise`]): they commit together, and a
/// refusal rolls back every write it made before refusing.
pub(crate) fn run(command: Command, ctx: &Context) -> Result<Output, CliError> {
    let meta = command.audit();
    let mut audit = meta.map(|meta| AuditGuard::start(ctx.store(), meta, ctx.now()));
    if let Some(audit) = audit.as_mut() {
        audit.set_committed_flag(ctx.audit_committed_flag());
    }
    let subject = command.subject(ctx);
    if let Some(audit) = audit.as_mut() {
        subject.apply(audit);
    }
    let finish = |audit: &mut Option<AuditGuard<'_>>, result: &Result<Output, CliError>| {
        if let Some(audit) = audit.as_mut() {
            audit.mark_result(result);
            subject.apply(audit);
            audit.set_instruction(ctx.instruction_note());
        }
    };
    if meta.is_none_or(|meta| meta.stepwise) {
        let result = dispatch(command, ctx);
        finish(&mut audit, &result);
        return result;
    }
    let result = ctx.store().transaction(|| {
        let result = dispatch(command, ctx);
        finish(&mut audit, &result);
        let output = result?;
        if let Some(audit) = audit.as_mut() {
            audit.flush()?;
        }
        Ok(output)
    });
    if result.is_err() {
        // The lines flushed inside rolled back with the transaction: the
        // guard writes the failure after it.
        finish(&mut audit, &result);
    }
    result
}

fn dispatch(command: Command, ctx: &Context) -> Result<Output, CliError> {
    match command {
        Command::Init(args) => init::run(args, ctx),
        Command::Group(command) => group::run(command, ctx),
        Command::Entry(command) => entry::run(command, ctx),
        Command::Session(command) => session::run(command, ctx),
        Command::Transcript(command) => transcript::run(command, ctx),
        Command::Proposal(command) => proposal::run(command, ctx),
        Command::Memory(command) => memory::run(command, ctx),
        Command::Search(args) => search::run(args, ctx),
        Command::Ask(args) => question::run(QuestionCommand::Ask(args), ctx),
        Command::Consistency(command) => consistency::run(command, ctx),
        Command::Store(command) => store::run(command, ctx),
        Command::Export(args) => export::run(&args, ctx),
        Command::Tag(command) => tag::run(command, ctx),
        Command::Hook(command) => hook::run(command, ctx),
        Command::Question(command) => question::run(command, ctx),
        Command::Brief(args) => brief::run(&args, ctx),
        Command::Tmux(args) => tmux::run(args, ctx),
        Command::Plugin(command) => plugin::run(command, ctx),
    }
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
                issue: None,
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
