use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::memory::{Command, exec};
use scribe_core::composition::Context;

pub(super) const REFRESH_EXAMPLES: &str = "\
Runs the deterministic part of memory refresh: replays any open memory
journal, archives legacy format-24 items, and succeeds with nothing_new when
there are no new sessions, profile edits or passed now items. When a model
rebuild is needed, points you to 'orbit job run scribe-memory-refresh' (or
'scribe memory prepare --json' and 'scribe memory apply' for an interactive
agent).

Examples:
  scribe memory refresh
  scribe memory refresh --json";

/// Arguments to `memory refresh`: none.
#[derive(Debug, Args)]
pub(crate) struct RefreshArgs {}

pub(super) fn run(RefreshArgs {}: RefreshArgs, ctx: &Context) -> Result<Output, CliError> {
    Ok(output(exec(ctx, Command::Refresh)?, ctx))
}
