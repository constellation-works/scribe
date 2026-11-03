//! Command dispatch: the noun enum, the one chokepoint every command runs
//! through, and one module per noun.
//!
//! [`run`] is where cross-cutting behaviour attaches, once, for every command
//! (STD-02 R24): today the audit log, through [`crate::audit_middleware`].
//! Each noun lives in its own module with its subcommands, flags, help and
//! handlers (`entry.rs`, `session.rs`, `proposal.rs`, `tag.rs`); a noun that
//! outgrows one file becomes a directory, `<noun>/mod.rs` plus one file per
//! verb, as `hook/` is (STD-02 R18: split along responsibilities, not by length alone).

mod entry;
mod hook;
mod proposal;
mod session;
mod tag;

use crate::app::Context;
use crate::audit_middleware::{AuditGuard, AuditMeta};
use crate::error::CliError;
use crate::output::{Output, Records, WarningPayload};
use clap::Subcommand;
use scribe_core::Skipped;

pub(crate) use entry::EntryCommand;
pub(crate) use hook::HookCommand;
pub(crate) use proposal::ProposalCommand;
pub(crate) use session::SessionCommand;
pub(crate) use tag::TagCommand;

/// Top-level nouns.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// List and show the entries recorded with #scribe
    #[command(subcommand, after_help = entry::ENTRY_EXAMPLES)]
    Entry(EntryCommand),
    /// List and show the records of sessions cataloged with /scribe on
    #[command(subcommand, after_help = session::SESSION_EXAMPLES)]
    Session(SessionCommand),
    /// List what the consistency routine proposes changing
    #[command(subcommand, after_help = proposal::PROPOSAL_EXAMPLES)]
    Proposal(ProposalCommand),
    /// List the tags entries are filed under
    #[command(subcommand, after_help = tag::TAG_EXAMPLES)]
    Tag(TagCommand),
    /// Run a Claude Code hook: record #scribe lines and fill in their cites
    #[command(subcommand, after_help = hook::HOOK_EXAMPLES)]
    Hook(HookCommand),
}

impl Command {
    /// What the audit log records for this command, or `None` when it is not
    /// audited. Only commands that change durable state are: a read-only
    /// command must not write, not even an audit line (STD-01 R31). The
    /// `hook` verbs are the writers. Exhaustive, so a new command cannot
    /// compile without deciding (STD-02 R27).
    pub(crate) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::Entry(command) => command.audit(),
            Self::Session(command) => command.audit(),
            Self::Proposal(command) => command.audit(),
            Self::Tag(command) => command.audit(),
            Self::Hook(command) => command.audit(),
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
        Command::Tag(command) => tag::run(command, ctx),
        Command::Hook(command) => hook::run(command, ctx),
    };
    if let Some(audit) = audit.as_mut() {
        audit.mark_result(&result);
    }
    result
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
