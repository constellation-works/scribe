use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::memory::{Command, exec};
use scribe_core::composition::Context;

pub(super) const RESTORE_EXAMPLES: &str = "\
Writes a stored profile revision back as the profile, as a new revision.

Examples:
  scribe memory show --revision 3
  scribe memory restore 3";

/// Arguments to `memory restore`.
#[derive(Debug, Args)]
pub(crate) struct RestoreArgs {
    /// The revision to write back, as `memory show --revision` names it
    #[arg(value_name = "N")]
    revision: i64,
}

pub(super) fn run(args: RestoreArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Restore {
        revision: args.revision,
    };
    Ok(output(exec(ctx, command)?, ctx))
}
