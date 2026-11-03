//! The `entry` noun: list and show the entries Daniel recorded with
//! `#scribe`. Read-only: entries are written only by the `UserPromptSubmit`
//! hook. Each verb makes one store call and wraps the result in a payload
//! (STD-02 R2); rendering happens later, in `output`, from the payload alone.

use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{EntryPayload, Output, Record, Records};
use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Args, Subcommand};
use scribe_core::{EntryFilter, EntryId, EntryStatus, Project, TagName, Topic};
use std::num::NonZeroUsize;

pub(super) const ENTRY_EXAMPLES: &str = "\
Examples:
  scribe entry list --tag decisions
  scribe entry list --status active --project scribe
  scribe entry show <id>";

const LIST_EXAMPLES: &str = "\
Examples:
  scribe entry list
  scribe entry list --tag preferences --status active
  scribe entry list --topic ci --limit 10
  scribe entry list --json | jq '.entries[].content'
  scribe entry list | cut -f1,6";

const SHOW_EXAMPLES: &str = "\
Examples:
  scribe entry show <id>
  scribe entry show <id> --json";

/// Verbs on the `entry` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum EntryCommand {
    /// List entries, oldest first
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
    /// Show one entry in full: what was said, its content and its cite
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
}

/// Arguments to `entry list`.
#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Only entries filed under this tag, by its name as `tag list` prints it [default: any]
    #[arg(long, value_name = "TAG")]
    pub(crate) tag: Option<TagName>,

    /// Only entries with this status [default: any]
    #[arg(long, value_name = "STATUS", value_parser = status_parser())]
    pub(crate) status: Option<EntryStatus>,

    /// Only entries recorded in this project (a repository slug, or global) [default: any]
    #[arg(long, value_name = "PROJECT")]
    pub(crate) project: Option<Project>,

    /// Only entries carrying this topic [default: any]
    #[arg(long, value_name = "TOPIC")]
    pub(crate) topic: Option<Topic>,

    /// Show at most this many of the matching entries, 1 or more; --json reports the total and whether the list was cut [default: all]
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<NonZeroUsize>,
}

/// Arguments to `entry show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The entry's id, as printed by `entry list`
    #[arg(value_name = "ID")]
    pub(crate) id: EntryId,
}

/// Allowed statuses come from the domain type, so help and parsing cannot
/// drift from it (STD-01 R25).
fn status_parser() -> impl TypedValueParser<Value = EntryStatus> {
    PossibleValuesParser::new(EntryStatus::NAMES).try_map(|s| s.parse::<EntryStatus>())
}

impl EntryCommand {
    /// The audit entry for this verb: `None`, both verbs read (STD-01 R31).
    /// See [`super::Command::audit`].
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) | Self::Show(_) => None,
        }
    }
}

/// Run one `entry` verb.
pub(super) fn run(command: EntryCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        EntryCommand::List(args) => list(args, ctx),
        EntryCommand::Show(args) => show(&args, ctx),
    }
}

fn list(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let filter = EntryFilter {
        tag: args.tag,
        status: args.status,
        project: args.project,
        topic: args.topic,
        limit: args.limit,
    };
    let found = ctx.store().list_entries(&filter)?;
    let empty = format!(
        "no entries in {}; they are recorded by a #scribe line in a Claude Code session",
        ctx.store().root().display()
    );
    let notice = ListNotice {
        noun: "entries",
        criteria: criteria(&filter),
        empty: &empty,
    };
    let records = Records::Entries(found.listing.items.iter().map(EntryPayload::from).collect());
    Ok(list_output(
        records,
        found.listing.total,
        &found.skipped,
        &notice,
    ))
}

/// The filters applied, as the flags that set them.
pub(super) fn criteria(filter: &EntryFilter) -> Vec<String> {
    let mut criteria = Vec::new();
    if let Some(tag) = &filter.tag {
        criteria.push(format!("--tag {tag}"));
    }
    if let Some(status) = filter.status {
        criteria.push(format!("--status {status}"));
    }
    if let Some(project) = &filter.project {
        criteria.push(format!("--project {project}"));
    }
    if let Some(topic) = &filter.topic {
        criteria.push(format!("--topic {topic}"));
    }
    criteria
}

fn show(args: &ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    let entry = ctx.store().entry(&args.id)?;
    Ok(Output::Show {
        record: Record::Entry(Box::new(EntryPayload::from(&entry))),
    })
}
