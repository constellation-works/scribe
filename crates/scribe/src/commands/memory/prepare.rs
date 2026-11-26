use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::memory::{Command, exec};
use scribe_core::composition::Context;

pub(super) const PREPARE_EXAMPLES: &str = "\
Replays any open memory journal and prints the bounded inputs, optimistic
basis, cursor and passed now stems needed to rebuild memory/profile.md.
Returns nothing_new: true when there is nothing to refresh.

Examples:
  scribe memory prepare --json";

/// Arguments to `memory prepare`: none.
#[derive(Debug, Args)]
pub(crate) struct PrepareArgs {}

pub(super) fn run(PrepareArgs {}: PrepareArgs, ctx: &Context) -> Result<Output, CliError> {
    Ok(output(exec(ctx, Command::Prepare)?, ctx))
}
