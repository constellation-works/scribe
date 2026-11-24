//! Configured and store tags: listing and audited direct writes.

use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{Output, Record, Records, TagPayload, TagShowPayload};
use clap::{Args, Subcommand};
use scribe_core::TagVerbs;

pub(super) const TAG_EXAMPLES: &str = "\
Examples:
  scribe tag list
  scribe tag list --json | jq -r '.tags[].name'
  scribe tag show preferences";

const SHOW_EXAMPLES: &str = "\
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

const LIST_EXAMPLES: &str = "\
Examples:
  scribe tag list
  scribe tag list --json";

/// Verbs on the `tag` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum TagCommand {
    /// Create an agent-owned tag in the store
    #[command(
        after_help = "Examples:\n  scribe tag add notes --author opus --description 'Agent notes'"
    )]
    Add(AddArgs),
    /// Update an agent-owned store tag
    ///
    /// Updates a store tag; configured tags remain in config.toml.
    #[command(after_help = "Examples:\n  scribe tag update notes --author opus --add-alias note")]
    Update(UpdateArgs),
    /// List config and store tags, their aliases, sources and entry counts
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
    /// Show one tag's settings and capture rules, with each field's origin
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
}

/// Arguments to `tag show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The tag's name or one of its aliases
    tag: String,
}

/// Arguments to `tag list` (none yet).
#[derive(Debug, Args)]
pub(crate) struct ListArgs {}

impl TagCommand {
    /// Audit every write; listings remain read-only.
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) | Self::Show(_) => None,
            Self::Add(_) => Some(AuditMeta {
                command: "tag add",
                stepwise: false,
            }),
            Self::Update(_) => Some(AuditMeta {
                command: "tag update",
                stepwise: false,
            }),
        }
    }
}

/// Run one `tag` verb.
pub(super) fn run(command: TagCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        TagCommand::List(ListArgs {}) => list(ctx),
        TagCommand::Show(args) => show(&args, ctx),
        TagCommand::Add(args) => {
            let now = ctx.now();
            let tag = scribe_core::StoreTag {
                name: args.name,
                aliases: args.alias,
                description: args.description,
                stale_after_days: args.stale_after_days,
                author: scribe_core::Author::agent(args.author),
                updated_by: None,
                created: now,
                updated: now,
            };
            // Arguments first: a bad or repeated tag is reported before an
            // unset [redact] env name.
            scribe_core::check_agent_author(&tag.author.name)?;
            ctx.store().check_new_tag(&tag)?;
            let redactor = ctx.redactor(&ctx.store().config()?)?;
            let tag = ctx.store().add_agent_tag(tag, &redactor)?;
            Ok(Output::written(Record::Tag(Box::new(tag)), true))
        }
        TagCommand::Update(args) => {
            scribe_core::check_agent_author(&args.author)?;
            if let Some(description) = &args.description {
                scribe_core::check_description(description)?;
            }
            let patch = scribe_core::TagPatch {
                description: args.description,
                add_aliases: args.add_alias,
                remove_aliases: args.remove_alias,
                stale_after_days: args.stale_after_days,
            };
            ctx.store().check_agent_tag_update(&args.name, &patch)?;
            let redactor = ctx.redactor(&ctx.store().config()?)?;
            let (tag, changed) = ctx.store().update_agent_tag(
                &args.name,
                patch,
                args.author,
                ctx.now(),
                &redactor,
            )?;
            Ok(Output::written(Record::Tag(Box::new(tag)), changed))
        }
    }
}

/// Read-only: no lock, no audit, no write.
fn show(args: &ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    let (def, source) = ctx.store().tag_definition(&args.tag)?;
    let file = ctx.store().config_path().display().to_string();
    Ok(Output::Show {
        record: Record::TagRules(Box::new(TagShowPayload::new(&def, source, &file))),
    })
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
    let mut output = list_output(records, total, &found.skipped, &notice);
    if let Output::List { notices, .. } = &mut output {
        notices.extend(found.warnings);
    }
    Ok(output)
}

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
