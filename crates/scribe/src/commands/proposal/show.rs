use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::proposal::{Command, exec};
use scribe_core::composition::Context;

pub(super) const SHOW_EXAMPLES: &str = "\
Examples:
  scribe proposal show p-7
  scribe proposal show p-7 --json | jq '.candidate.context'";

/// Arguments to `proposal show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The proposal: its id (`p-<n>`)
    pub(crate) id: String,
}

pub(super) fn run(args: ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    Ok(output(exec(ctx, Command::Show(args.id))?, None, ctx))
}
