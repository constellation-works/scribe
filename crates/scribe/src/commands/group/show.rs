use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::group::{Command, exec};
use scribe_core::composition::Context;

pub(super) const SHOW_EXAMPLES: &str = "Example: scribe group show <id> --json";

/// Arguments to `group show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// Group id, unique prefix, or project group slug
    id: String,
}

pub(super) fn run(args: ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    Ok(output(exec(ctx, Command::Show(args.id))?))
}
