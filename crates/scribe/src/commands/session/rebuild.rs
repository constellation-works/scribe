//! Rebuild a record from the first transcript message. Each window is written
//! to an isolated dot-file; only the complete rebuild replaces the record.

use super::{once, passed};
use crate::error::CliError;
use crate::output::Output;
use clap::Args;
use scribe_core::apps::session::{self as app, Command, RebuildInput, exec};
use scribe_core::composition::Context;
use scribe_core::{Audit, AuthorName, Config, Models, SessionId};

pub(in crate::commands) const COMMAND: &str = app::REBUILD_COMMAND;
pub(super) const EXAMPLES: &str = "\
Replaces the header and log only after every transcript window succeeds.
Interrupted runs resume their private checkpoint; --confirm is required.

Examples:
  scribe session rebuild <id> --author <name> --confirm
  scribe session rebuild <id> --author <name> --confirm --json";

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

pub(super) fn run(
    args: &RebuildArgs,
    audit: Option<Audit>,
    ctx: &Context,
) -> Result<Output, CliError> {
    run_audited(args, audit, ctx, |config| ctx.models(config))
}

/// [`run`] with the `Models` adapter `models` builds for the config.
fn run_audited<M: Models>(
    args: &RebuildArgs,
    audit: Option<Audit>,
    ctx: &Context,
    models: impl FnOnce(&Config) -> M,
) -> Result<Output, CliError> {
    let input = RebuildInput {
        id: &args.id,
        author: &args.author,
        confirm: args.confirm,
        models: once(models),
    };
    Ok(passed(exec(ctx, audit, Command::Rebuild(input))?, ctx))
}
