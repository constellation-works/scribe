//! Command dispatch: the noun enum, the one chokepoint every command runs
//! through, and one module per noun.
//!
//! [`run`] is where cross-cutting behaviour attaches, once, for every command
//! (STD-02 R24): today the audit log, through [`crate::audit_middleware`].
//! Each noun lives in its own module with its subcommands, flags, help and
//! handlers (`entry.rs`, `session/`, `proposal.rs`, `tag.rs`, `question.rs`,
//! `plugin.rs`); a noun that
//! outgrows one file becomes a directory, `<noun>/mod.rs` plus one file per
//! verb, as `hook/` is (STD-02 R18: split along responsibilities, not by length alone).

mod consistency;
mod entry;
mod hook;
mod plugin;
mod proposal;
mod question;
mod session;
mod tag;

use crate::app::Context;
use crate::audit_middleware::{AuditGuard, AuditMeta};
use crate::error::CliError;
use crate::output::{Output, Records, WarningPayload};
use clap::Subcommand;
use scribe_core::Skipped;

pub(crate) use consistency::ConsistencyCommand;
pub(crate) use entry::EntryCommand;
pub(crate) use hook::HookCommand;
pub(crate) use plugin::PluginCommand;
pub(crate) use proposal::ProposalCommand;
pub(crate) use question::QuestionCommand;
pub(crate) use session::SessionCommand;
pub(crate) use tag::TagCommand;

/// Top-level nouns.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// List and show the entries recorded with #scribe
    #[command(subcommand, after_help = entry::ENTRY_EXAMPLES)]
    Entry(EntryCommand),
    /// List and show the records of sessions cataloged with /scribe on, and write them (pass)
    #[command(subcommand, after_help = session::SESSION_EXAMPLES)]
    Session(SessionCommand),
    /// List what the consistency routine proposes changing
    #[command(subcommand, after_help = proposal::PROPOSAL_EXAMPLES)]
    Proposal(ProposalCommand),
    /// Propose superseding or retiring entries a later one replaced or that are due for review (scan)
    #[command(subcommand, after_help = consistency::CONSISTENCY_EXAMPLES)]
    Consistency(ConsistencyCommand),
    /// List the tags entries are filed under
    #[command(subcommand, after_help = tag::TAG_EXAMPLES)]
    Tag(TagCommand),
    /// Answer a question from the records, citing a verbatim quote for every claim
    #[command(subcommand, after_help = question::QUESTION_EXAMPLES)]
    Question(QuestionCommand),
    /// Run a Claude Code hook: record #scribe lines and settle proposals, switch /scribe on and off, mark session passes due, report open proposals
    #[command(subcommand, after_help = hook::HOOK_EXAMPLES)]
    Hook(HookCommand),
    /// Serve the Orbit plugin's tools (the plugin backend runs this)
    #[command(subcommand, after_help = plugin::PLUGIN_EXAMPLES)]
    Plugin(PluginCommand),
}

impl Command {
    /// What the audit log records for this command, or `None` when it is not
    /// audited. Only commands that change durable state are: a read-only
    /// command must not write, not even an audit line (STD-01 R31). The
    /// `hook` verbs, `session pass` and `consistency scan` are the writers. Exhaustive, so a new command cannot
    /// compile without deciding (STD-02 R27).
    pub(crate) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::Entry(command) => command.audit(),
            Self::Session(command) => command.audit(),
            Self::Proposal(command) => command.audit(),
            Self::Consistency(command) => command.audit(),
            Self::Tag(command) => command.audit(),
            Self::Hook(command) => command.audit(),
            Self::Question(command) => command.audit(),
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
    let result = match command {
        Command::Entry(command) => entry::run(command, ctx),
        Command::Session(command) => session::run(command, ctx),
        Command::Proposal(command) => proposal::run(command, ctx),
        Command::Consistency(command) => consistency::run(command, ctx),
        Command::Tag(command) => tag::run(command, ctx),
        Command::Hook(command) => hook::run(command, ctx),
        Command::Question(command) => question::run(command, ctx),
        Command::Plugin(command) => plugin::run(command, ctx),
    };
    if let Some(audit) = audit.as_mut() {
        audit.mark_result(&result);
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
}

/// What a list needs to say on stderr beside its records.
pub(super) struct ListNotice<'a> {
    /// The plural noun, such as `entries`.
    pub(super) noun: &'static str,
    /// The filters applied, as the flags that set them (`--tag decisions`).
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

#[cfg(test)]
mod tests;
