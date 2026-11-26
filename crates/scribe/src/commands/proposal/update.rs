use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::proposal::{Command, exec};
use scribe_core::composition::Context;

pub(super) const UPDATE_EXAMPLES: &str =
    "Examples:\n  scribe proposal update <id> --author opus --reason 'New evidence'";

/// Arguments to `proposal update`.
#[derive(Debug, Args)]
pub(crate) struct UpdateArgs {
    /// Proposal id or unique prefix (at least four characters)
    id: String,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Replacement reason
    #[arg(long)]
    reason: String,
}

pub(super) fn run(args: UpdateArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Update {
        id: args.id,
        reason: args.reason,
        author: args.author,
    };
    Ok(output(exec(ctx, command)?, None, ctx))
}
