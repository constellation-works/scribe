use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::group::{Command, exec};
use scribe_core::composition::Context;

pub(super) const LEAVE_EXAMPLES: &str = "Example: scribe group leave";

/// Arguments to `group leave`: none.
#[derive(Debug, Args)]
pub(crate) struct LeaveArgs {}

pub(super) fn run(LeaveArgs {}: LeaveArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Leave {
        session: ctx.caller(),
        provider: ctx.provider(),
    };
    Ok(output(exec(ctx, command)?))
}
