use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::tag::{Command, exec};
use scribe_core::composition::Context;

pub(super) const UPDATE_EXAMPLES: &str =
    "Examples:\n  scribe tag update notes --author opus --add-alias note";

/// Arguments to `tag update`.
#[derive(Debug, Args)]
pub(crate) struct UpdateArgs {
    /// Canonical store tag name
    name: scribe_core::TagName,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Replacement description
    #[arg(long)]
    description: Option<String>,
    /// Alias to add, repeatable
    #[arg(long)]
    add_alias: Vec<scribe_core::TagName>,
    /// Alias to remove, repeatable
    #[arg(long)]
    remove_alias: Vec<scribe_core::TagName>,
    /// Replacement review interval in days (positive)
    #[arg(long)]
    stale_after_days: Option<std::num::NonZeroU32>,
}

pub(super) fn run(args: UpdateArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Update {
        name: args.name,
        patch: scribe_core::TagPatch {
            description: args.description,
            add_aliases: args.add_alias,
            remove_aliases: args.remove_alias,
            stale_after_days: args.stale_after_days,
        },
        author: args.author,
    };
    Ok(output(exec(ctx, command)?))
}
