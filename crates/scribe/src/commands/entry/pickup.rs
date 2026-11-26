use super::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::entry::{self, Command, exec};
use scribe_core::composition::Context;

pub(super) const PICKUP_EXAMPLES: &str = "\
Examples:
  scribe entry pickup <id> --author standards-intake --result task:<id>
  scribe entry pickup <id> --author standards-intake --rejected";

#[derive(Debug, Args)]
pub(crate) struct PickupArgs {
    /// The action entry's id or a unique prefix (at least four characters)
    id: String,
    /// The consumer's name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// What the consumer made of it (task:<id>, pr:<owner>/<repo>#<n>, repo:<slug> or session:<id>, whose id may be a unique prefix)
    #[arg(long, value_name = "REF")]
    result: Option<scribe_core::Related>,
    /// The user rejected the result
    #[arg(long)]
    rejected: bool,
}

pub(super) fn run(args: PickupArgs, ctx: &Context) -> Result<Output, CliError> {
    let PickupArgs {
        id,
        author,
        result,
        rejected,
    } = args;
    let input = entry::PickupInput {
        id: &id,
        author,
        result: result.as_ref(),
        rejected,
        provider: ctx.provider(),
    };
    let command = Command::Pickup(input);
    Ok(output::output(exec(ctx, command)?, false, None, ctx))
}
