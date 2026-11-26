use super::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::entry::{Command, exec};
use scribe_core::composition::Context;

pub(super) const SHOW_EXAMPLES: &str = "\
Examples:
  scribe entry show <id>
  scribe entry show <id> --window
  scribe entry show <id> --json";

/// Arguments to `entry show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The entry's id, as printed by `entry list`, or a unique prefix of it (at least four characters)
    #[arg(value_name = "ID")]
    pub(crate) id: String,

    /// Include the entry's transcript window in full [default: only its size]
    #[arg(long)]
    pub(crate) window: bool,
}

pub(super) fn run(args: ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    Ok(output::output(
        exec(ctx, Command::Show(args.id))?,
        args.window,
        None,
        ctx,
    ))
}
