//! Quote-checked group opening, caller-bound seat membership and roster reads.

mod backfill;
mod close;
mod evidence;
mod join;
mod leave;
mod list;
mod open;
mod outcome;
mod show;

use backfill::BackfillArgs;
use close::CloseArgs;
use evidence::EvidenceArgs;
use join::JoinArgs;
use leave::LeaveArgs;
use list::ListArgs;
use open::OpenArgs;
use show::ShowArgs;

use crate::{error::CliError, output::Output};
use clap::Subcommand;
use scribe_core::composition::Context;

pub(super) const EXAMPLES: &str = "Examples:
  scribe group open --title 'Release review' --kind review --project scribe --quote 'open a release review'
  scribe group join <id> --as editor --author aide --role reviewer
  scribe group close <id> --summary 'Review complete'
  scribe group show <id> --json";

/// Verbs on the `group` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum GroupCommand {
    /// Open a group on a verbatim quote from the calling session
    #[command(after_help = open::OPEN_EXAMPLES)]
    Open(OpenArgs),
    /// Join a named seat under your own author name
    #[command(after_help = join::JOIN_EXAMPLES)]
    Join(JoinArgs),
    /// Store earlier typed turns for each current seat holder
    #[command(after_help = backfill::BACKFILL_EXAMPLES)]
    Backfill(BackfillArgs),
    /// Remove the calling session from its seat
    #[command(after_help = leave::LEAVE_EXAMPLES)]
    Leave(LeaveArgs),
    /// Summarize and close the group from its opener, seat holder or a quote
    #[command(after_help = close::CLOSE_EXAMPLES)]
    Close(CloseArgs),
    /// Read bounded evidence for drafting a close summary
    Evidence(EvidenceArgs),
    /// Show one group, its references and its seat chains
    #[command(after_help = show::SHOW_EXAMPLES)]
    Show(ShowArgs),
    /// List groups newest first with their rosters
    #[command(after_help = list::LIST_EXAMPLES)]
    List(ListArgs),
}

impl GroupCommand {
    /// Whether this is `group close` with neither `--summary` nor `--no-summary`.
    pub(crate) fn lacks_close_summary(&self) -> bool {
        matches!(self, Self::Close(args) if args.lacks_summary())
    }
}

pub(super) fn run(command: GroupCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        GroupCommand::Open(args) => open::run(args, ctx),
        GroupCommand::Join(args) => join::run(args, ctx),
        GroupCommand::Backfill(args) => backfill::run(args, ctx),
        GroupCommand::Leave(args) => leave::run(args, ctx),
        GroupCommand::Close(args) => close::run(args, ctx),
        GroupCommand::Evidence(args) => evidence::run(args, ctx),
        GroupCommand::Show(args) => show::run(args, ctx),
        GroupCommand::List(args) => list::run(args, ctx),
    }
}
