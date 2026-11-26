use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::group::{Command, exec};
use scribe_core::composition::Context;

pub(super) const LIST_EXAMPLES: &str = "Example: scribe group list --json";

/// Arguments to `group list`: none.
#[derive(Debug, Args)]
pub(crate) struct ListArgs {}

pub(super) fn run(ListArgs {}: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    Ok(output(exec(ctx, Command::List)?))
}
