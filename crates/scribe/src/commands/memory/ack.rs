use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::memory::{Command, exec};
use scribe_core::composition::Context;
use scribe_core::{AuthorName, SessionId};

pub(super) const ACK_EXAMPLES: &str = "\
Run it after raising a now item the user did not bring up themselves, so the
next session does not raise it again unprompted.

Examples:
  scribe memory ack 2026-12-01-hackathon-deadline --session <id> --author opus";

/// Arguments to `memory ack`.
#[derive(Debug, Args)]
pub(crate) struct AckArgs {
    /// The now item's file stem
    id: String,
    /// The session the item was raised in (an id or a unique prefix of it)
    #[arg(long, value_name = "ID")]
    session: SessionId,
    /// Your crew or model name
    #[arg(long, value_name = "NAME")]
    author: AuthorName,
}

pub(super) fn run(args: AckArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Ack {
        id: args.id,
        session: args.session,
        author: args.author,
        provider: ctx.provider(),
    };
    Ok(output(exec(ctx, command)?, ctx))
}
