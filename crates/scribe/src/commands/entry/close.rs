use super::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::EntryId;
use scribe_core::apps::entry::{Command, exec};
use scribe_core::composition::Context;

pub(super) const CLOSE_EXAMPLES: &str = "\
The asker closes an item once it has carried the answer out. A choice closes
only once answered; an action item closes answered or not.

Examples:
  scribe entry close <id> --author opus --decision <id>
  scribe entry close <id> --author opus --note 'Restarted; nothing lasting'";

#[derive(Debug, Args)]
#[command(group = clap::ArgGroup::new("settled").required(true).multiple(true).args(["decision", "note"]))]
pub(crate) struct CloseArgs {
    /// The open item's id or a unique prefix (at least four characters)
    id: String,
    /// Agent name of the asker closing it
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// The active decision written for this item with --settles
    #[arg(long, value_name = "ID")]
    decision: Option<EntryId>,
    /// What was done, when no lasting decision came of it (at most 300 characters)
    #[arg(long)]
    note: Option<String>,
}

pub(super) fn run(args: CloseArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Close {
        id: args.id,
        decision: args.decision,
        note: args.note,
        author: args.author,
    };
    Ok(output::output(exec(ctx, command)?, false, None, ctx))
}
