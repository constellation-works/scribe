use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::memory::{Command, exec};
use scribe_core::composition::Context;

pub(super) const SHOW_EXAMPLES: &str = "\
With no id, prints the memory block the session-start brief embeds:
Profile, Now (every item, nearest date first) and Milestones (the last
five, newest first). With an id (profile, or a now or milestone file stem),
prints that file. --revision prints one stored profile revision.

Examples:
  scribe memory show
  scribe memory show --json | jq '.now[].id'
  scribe memory show profile
  scribe memory show 2026-12-01-hackathon-deadline --json
  scribe memory show --revision 3";

/// Arguments to `memory show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// Show this file: profile, or a now or milestone file stem [default: the memory block]
    #[arg(value_name = "ID")]
    id: Option<String>,

    /// Show this stored profile revision instead
    #[arg(long, value_name = "N", conflicts_with = "id")]
    revision: Option<i64>,
}

pub(super) fn run(args: ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Show {
        revision: args.revision,
        id: args.id,
    };
    Ok(output(exec(ctx, command)?, ctx))
}
