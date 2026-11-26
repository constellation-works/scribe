use super::outcome::output;
use crate::commands::instruction::InstructionArgs;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::proposal::{self, Command, exec};
use scribe_core::composition::Context;

pub(super) const SETTLE_EXAMPLES: &str = "Examples:\n  scribe proposal settle <id> --approve --author opus\n  scribe proposal settle <id> --reject --author opus --session <id> --instruction '<their words>'";

/// Arguments to `proposal settle`.
#[derive(Debug, Args)]
pub(crate) struct SettleArgs {
    /// Proposal id or unique prefix
    id: String,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Apply the proposal
    #[arg(long, required_unless_present = "reject", conflicts_with = "reject")]
    approve: bool,
    /// Close without applying
    #[arg(long, required_unless_present = "approve")]
    reject: bool,
    /// Replacement explanation for the settlement
    #[arg(long)]
    reason: Option<String>,
    #[command(flatten)]
    instruction: InstructionArgs,
}

pub(super) fn run(args: SettleArgs, ctx: &Context) -> Result<Output, CliError> {
    let input = proposal::ProposalSettleInput {
        id: &args.id,
        approve: args.approve,
        reason: args.reason.as_deref(),
        author: args.author,
        instruction: args.instruction.case(ctx),
    };
    Ok(output(exec(ctx, Command::Settle(input))?, None, ctx))
}
