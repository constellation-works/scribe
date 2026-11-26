//! `scribe memory`: what scribe remembers about the user — the profile,
//! `now` items and milestones (docs/design/scribe/2_design.md §15.4–§15.6,
//! §19.3.5).
//!
//! `show` and `rules` read only: no lock, no audit, no replay. `ack`,
//! `forget`, `restore`, `prepare`, `apply` and `refresh` are audited writes
//! and call no model. Memory is scribe-authored, so an agent acknowledges or
//! forgets an item without the user's instruction check, as it may an agent
//! note.

mod ack;
mod apply;
mod forget;
mod outcome;
mod prepare;
mod refresh;
mod restore;
mod rules;
mod show;

use ack::AckArgs;
use apply::ApplyArgs;
use forget::ForgetArgs;
use prepare::PrepareArgs;
use refresh::RefreshArgs;
use restore::RestoreArgs;
use rules::RulesArgs;
use show::ShowArgs;

use crate::{error::CliError, output::Output};
use clap::Subcommand;
use scribe_core::composition::Context;

pub(super) const MEMORY_EXAMPLES: &str = "\
Memory is one fixed shape: memory/profile.md, rebuilt weekly by the
scribe-memory-refresh Orbit job; memory/now/, dated things the user plans,
attends or must meet, written by the session pass of a session switched on
with /scribe on; and memory/milestones/. It describes, it never binds: rules
are their preferences entries.

Examples:
  scribe memory show
  scribe memory show profile
  scribe memory show 2026-12-01-hackathon-deadline
  scribe memory ack 2026-12-01-hackathon-deadline --session <id> --author opus
  scribe memory refresh
  scribe memory prepare --json";

/// Verbs on the `memory` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum MemoryCommand {
    /// Show the memory block, one memory file, or a profile revision
    ///
    /// Shows the memory block (Profile, Now, Milestones), one file by id, or one stored profile revision.
    #[command(after_help = show::SHOW_EXAMPLES)]
    Show(ShowArgs),
    /// Show memory's fixed rules
    ///
    /// Shows memory's fixed rules and values; memory has no switches.
    #[command(after_help = rules::RULES_EXAMPLES)]
    Rules(RulesArgs),
    /// Mark a now item as raised in this session
    ///
    /// Marks a now item as raised in this session, so it is not raised again unprompted.
    #[command(after_help = ack::ACK_EXAMPLES)]
    Ack(AckArgs),
    /// Forget a now item or milestone for good
    ///
    /// Removes a now item or milestone file for good, with its history.
    #[command(after_help = forget::FORGET_EXAMPLES)]
    Forget(ForgetArgs),
    /// Write a stored profile revision back as the profile
    #[command(after_help = restore::RESTORE_EXAMPLES)]
    Restore(RestoreArgs),
    /// Run deterministic refresh checks or point to the Orbit refresh job
    ///
    /// Replays any open memory journal, succeeds when nothing is new, and otherwise directs you to the scribe-memory-refresh Orbit job.
    #[command(after_help = refresh::REFRESH_EXAMPLES)]
    Refresh(RefreshArgs),
    /// Prepare bounded profile refresh inputs and optimistic basis
    ///
    /// Replays any open memory journal and emits the bounded inputs and basis for scribe_profile_refresh or an interactive agent.
    #[command(after_help = prepare::PREPARE_EXAMPLES)]
    Prepare(PrepareArgs),
    /// Validate and commit a rebuilt profile answer from stdin JSON
    ///
    /// Checks the optimistic basis, profile rules, redaction and milestone bounds, then commits the refresh atomically.
    #[command(after_help = apply::APPLY_EXAMPLES)]
    Apply(ApplyArgs),
}

/// Run one `memory` verb.
pub(super) fn run(command: MemoryCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        MemoryCommand::Show(args) => show::run(args, ctx),
        MemoryCommand::Rules(args) => rules::run(args, ctx),
        MemoryCommand::Ack(args) => ack::run(args, ctx),
        MemoryCommand::Forget(args) => forget::run(args, ctx),
        MemoryCommand::Restore(args) => restore::run(args, ctx),
        MemoryCommand::Refresh(args) => refresh::run(args, ctx),
        MemoryCommand::Prepare(args) => prepare::run(args, ctx),
        MemoryCommand::Apply(args) => apply::run(args, ctx),
    }
}
