use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::tag::{Command, TagAddInput, exec};
use scribe_core::composition::Context;

pub(super) const ADD_EXAMPLES: &str =
    "Examples:\n  scribe tag add notes --author opus --description 'Agent notes'";

/// Arguments to `tag add`.
#[derive(Debug, Args)]
pub(crate) struct AddArgs {
    /// Canonical tag name
    name: scribe_core::TagName,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// What belongs in the tag
    #[arg(long)]
    description: String,
    /// Alias, repeatable
    #[arg(long)]
    alias: Vec<scribe_core::TagName>,
    /// Days before review (positive; omitted means never)
    #[arg(long)]
    stale_after_days: Option<std::num::NonZeroU32>,
}

pub(super) fn run(args: AddArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Add(TagAddInput {
        name: args.name,
        aliases: args.alias,
        description: args.description,
        stale_after_days: args.stale_after_days,
        author: args.author,
    });
    Ok(output(exec(ctx, command)?))
}
