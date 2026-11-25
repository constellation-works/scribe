//! `scribe session rebuild`: check that a record can be rebuilt and name
//! the `scribe-session-rebuild` Orbit job, which rebuilds it window by
//! window into a private checkpoint and replaces the record only when the
//! whole transcript is in it (docs/design/scribe/2_design.md §19.3.4).

use super::passed;
use crate::error::CliError;
use crate::output::Output;
use clap::Args;
use scribe_core::apps::session::{Command, RebuildInput, exec};
use scribe_core::composition::Context;
use scribe_core::{AuthorName, SessionId};

pub(super) const EXAMPLES: &str = "\
Checks what the rebuild refuses (a forgotten session, a missing --confirm, a
session a pass owns, a missing or unproven transcript) and prints the Orbit
job command. The job replaces the header and log only after every transcript
window succeeds; a failed or interrupted run keeps its private checkpoint,
and running the job again resumes it.

Examples:
  scribe session rebuild <id> --author <name> --confirm
  orbit job run scribe-session-rebuild --input '{\"session\":\"<id>\",\"author\":\"<name>\",\"confirm\":true}'";

#[derive(Debug, Args)]
pub(crate) struct RebuildArgs {
    /// Session record to replace: its id or a unique prefix of it (at least four characters)
    #[arg(value_name = "ID")]
    pub(crate) id: SessionId,
    /// Name of the agent requesting the rebuild (lowercase letters, digits, '.', '_' or '-')
    #[arg(long, value_name = "NAME")]
    pub(crate) author: AuthorName,
    /// Confirm replacing the current header and log
    #[arg(long)]
    pub(crate) confirm: bool,
}

pub(super) fn run(args: &RebuildArgs, ctx: &Context) -> Result<Output, CliError> {
    let input = RebuildInput {
        id: &args.id,
        author: &args.author,
        confirm: args.confirm,
    };
    Ok(passed(exec(ctx, Command::Rebuild(input))?, ctx))
}
