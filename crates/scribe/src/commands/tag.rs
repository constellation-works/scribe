//! The `tag` noun: list the tags entries are filed under, from `config.toml`
//! (or the defaults), with their entry counts. Read-only: tags are added by
//! editing the config, never by a command.

use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{Output, Records, TagPayload};
use clap::{Args, Subcommand};

pub(super) const TAG_EXAMPLES: &str = "\
Examples:
  scribe tag list
  scribe tag list --json | jq -r '.tags[].name'";

const LIST_EXAMPLES: &str = "\
Examples:
  scribe tag list
  scribe tag list --json";

/// Verbs on the `tag` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum TagCommand {
    /// List the configured tags, their aliases and how many entries each holds
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
}

/// Arguments to `tag list` (none yet).
#[derive(Debug, Args)]
pub(crate) struct ListArgs {}

impl TagCommand {
    /// The audit entry for this verb: `None`, it reads (STD-01 R31).
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) => None,
        }
    }
}

/// Run one `tag` verb.
pub(super) fn run(command: TagCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        TagCommand::List(ListArgs {}) => list(ctx),
    }
}

fn list(ctx: &Context) -> Result<Output, CliError> {
    let found = ctx.store().list_tags()?;
    let notice = ListNotice {
        noun: "tags",
        criteria: Vec::new(),
        empty: "no tags: config.toml declares none, so no #scribe line can be filed",
    };
    let total = found.tags.len();
    let records = Records::Tags(found.tags.iter().map(TagPayload::from).collect());
    Ok(list_output(records, total, &found.skipped, &notice))
}
