//! Proposal listing and audited direct agent writes and settlement.

mod add;
mod list;
mod outcome;
mod settle;
mod show;
mod update;

use add::AddArgs;
use list::ListArgs;
use settle::SettleArgs;
use show::ShowArgs;
use update::UpdateArgs;

use crate::{error::CliError, output::Output};
use clap::Subcommand;
use scribe_core::composition::Context;

pub(super) const PROPOSAL_EXAMPLES: &str = "\
Examples:
  scribe proposal list
  scribe proposal show <id>
  scribe proposal list --json | jq '.proposals[] | select(.state == \"open\")'";

/// Verbs on the `proposal` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum ProposalCommand {
    /// Open a proposal, including against an entry of the user's
    #[command(after_help = add::ADD_EXAMPLES)]
    Add(AddArgs),
    /// Update the reason of an open agent proposal
    #[command(after_help = update::UPDATE_EXAMPLES)]
    Update(UpdateArgs),
    /// Approve or reject a proposal or candidate
    ///
    /// Approves or rejects a proposal about an agent entry, or a claim on the user's instruction.
    #[command(after_help = settle::SETTLE_EXAMPLES)]
    Settle(SettleArgs),
    /// List every proposal by id and its settlement state
    #[command(after_help = list::LIST_EXAMPLES)]
    List(ListArgs),
    /// Show one proposal in full: a claim's context, subject and quote
    #[command(after_help = show::SHOW_EXAMPLES)]
    Show(ShowArgs),
}

/// Run one `proposal` verb.
pub(super) fn run(command: ProposalCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        ProposalCommand::Add(args) => add::run(args, ctx),
        ProposalCommand::Update(args) => update::run(args, ctx),
        ProposalCommand::Settle(args) => settle::run(args, ctx),
        ProposalCommand::List(args) => list::run(args, ctx),
        ProposalCommand::Show(args) => show::run(args, ctx),
    }
}
