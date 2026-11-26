use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::group::{Command, exec};
use scribe_core::composition::Context;

pub(super) const BACKFILL_EXAMPLES: &str = "Example: scribe group backfill <id> --json";

/// Arguments to `group backfill`.
#[derive(Debug, Args)]
pub(crate) struct BackfillArgs {
    /// Group id, unique prefix, or project group slug
    id: String,
}

pub(super) fn run(args: BackfillArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Backfill {
        id: args.id,
        provider: ctx.provider(),
    };
    Ok(output(exec(ctx, command)?))
}
