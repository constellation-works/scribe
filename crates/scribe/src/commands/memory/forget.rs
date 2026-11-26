use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::AuthorName;
use scribe_core::apps::memory::{Command, exec};
use scribe_core::composition::Context;

pub(super) const FORGET_EXAMPLES: &str = "\
Removes a now item or milestone for good, with its history; it cannot be
undone, so it needs --confirm. The profile is corrected by editing
memory/profile.md: the next refresh takes the edit as the user's correction.

Examples:
  scribe memory forget 2026-12-01-hackathon-deadline --author opus --confirm";

/// Arguments to `memory forget`.
#[derive(Debug, Args)]
pub(crate) struct ForgetArgs {
    /// The now item's or milestone's file stem
    id: String,
    /// Your crew or model name
    #[arg(long, value_name = "NAME")]
    author: AuthorName,
    /// Confirm removing it for good; it cannot be undone
    #[arg(long)]
    confirm: bool,
}

pub(super) fn run(args: ForgetArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Forget {
        id: args.id,
        author: args.author,
        confirm: args.confirm,
    };
    Ok(output(exec(ctx, command)?, ctx))
}
