//! Configured and store tags: listing and audited direct writes.

mod add;
mod list;
mod outcome;
mod show;
mod update;

use add::AddArgs;
use list::ListArgs;
use show::ShowArgs;
use update::UpdateArgs;

use crate::{error::CliError, output::Output};
use clap::Subcommand;
use scribe_core::composition::Context;

pub(super) const TAG_EXAMPLES: &str = "\
Examples:
  scribe tag list
  scribe tag list --json | jq -r '.tags[].name'
  scribe tag show preferences";

/// Verbs on the `tag` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum TagCommand {
    /// Create an agent-owned tag in the store
    #[command(after_help = add::ADD_EXAMPLES)]
    Add(AddArgs),
    /// Update an agent-owned store tag
    ///
    /// Updates a store tag; configured tags remain in config.toml.
    #[command(after_help = update::UPDATE_EXAMPLES)]
    Update(UpdateArgs),
    /// List config and store tags, their aliases, sources and entry counts
    #[command(after_help = list::LIST_EXAMPLES)]
    List(ListArgs),
    /// Show one tag's settings and capture rules, with each field's origin
    #[command(after_help = show::SHOW_EXAMPLES)]
    Show(ShowArgs),
}

/// Run one `tag` verb.
pub(super) fn run(command: TagCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        TagCommand::Add(args) => add::run(args, ctx),
        TagCommand::Update(args) => update::run(args, ctx),
        TagCommand::List(args) => list::run(args, ctx),
        TagCommand::Show(args) => show::run(args, ctx),
    }
}
