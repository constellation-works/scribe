use super::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::entry::{Command, exec};
use scribe_core::composition::Context;

pub(super) const ANSWER_EXAMPLES: &str = "\
The quote is checked as a decision's is: verbatim in a typed turn of the user's
in a named session on this host; a quote under eight characters must be the
whole turn. A later answer replaces an earlier one until the item closes.

Examples:
  scribe entry answer <id> --author opus --quote '<their words>' --related session:<id>";

#[derive(Debug, Args)]
pub(crate) struct AnswerArgs {
    /// The open item's id or a unique prefix (at least four characters)
    id: String,
    /// Agent name of the session recording the answer
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// The user's answer, verbatim from a typed turn of theirs in a related session
    #[arg(long)]
    quote: String,
    /// The session the user typed the answer in, as session:<id> (a unique prefix is accepted); repeatable
    #[arg(long, value_name = "REF", required = true)]
    related: Vec<scribe_core::Related>,
}

pub(super) fn run(args: AnswerArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Answer {
        id: args.id,
        author: args.author,
        quote: args.quote,
        related: args.related,
        provider: ctx.provider(),
    };
    Ok(output::output(exec(ctx, command)?, false, None, ctx))
}
