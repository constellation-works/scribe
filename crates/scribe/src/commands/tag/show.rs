use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::tag::{Command, exec};
use scribe_core::composition::Context;

pub(super) const SHOW_EXAMPLES: &str = "\
Each capture field shows its origin: shipped (scribe's default), config
(config.toml, with its line) or none. Set fields under
[tags.<name>.capture] in config.toml: from, route, triggers, criteria,
not and max_open; each overrides its shipped default alone, and triggers
replaces the whole list. The session pass catches a claim under a tag only
from a turn of the speaker its from names (user, assistant or both) where
one of its triggers occurs (docs/design/scribe/specs/catchers.md).

Examples:
  scribe tag show preferences
  scribe tag show pref --json | jq '.capture.triggers'";

/// Arguments to `tag show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The tag's name or one of its aliases
    tag: String,
}

pub(super) fn run(args: ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    Ok(output(exec(ctx, Command::Show(args.tag))?))
}
