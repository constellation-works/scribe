use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::group::{Command, exec};
use scribe_core::composition::Context;

/// Arguments to `group evidence`.
#[derive(Debug, Args)]
pub(crate) struct EvidenceArgs {
    /// Group id, unique prefix, or project group slug
    id: String,
    /// Words the user typed in this session naming the group
    #[arg(long)]
    quote: Option<String>,
}

pub(super) fn run(args: EvidenceArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Evidence {
        session: ctx.caller(),
        provider: ctx.provider(),
        id: args.id,
        quote: args.quote,
    };
    Ok(output(exec(ctx, command)?))
}
