//! Entry listing, display and audited direct agent writes.

use super::{DateArgs, ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{EntryPayload, Output, Record, Records};
use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Args, Subcommand};
use scribe_core::{
    AuthorKind, AuthorName, EntryFilter, EntryStatus, Project, Related, TagName, Topic,
};
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
  scribe entry list --since 2026-10-01 --until 2026-10-31 --author-kind daniel
  scribe entry list --related task:<id> --related pr:<owner>/<repo>#<n>
  scribe entry list --json | jq '.entries[].content'
  scribe entry list | cut -f1,6";

const SHOW_EXAMPLES: &str = "\
Examples:
  scribe entry show <id>
  scribe entry show <id> --json";

/// Verbs on the `entry` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum EntryCommand {
    /// Add an agent note (never attributed to Daniel)
    #[command(
        after_help = "Examples:\n  scribe entry add --tag decisions --author opus 'Use the stable API'"
    )]
    Add(AddArgs),
    /// Update content or metadata; Daniel's entries allow metadata only
    #[command(after_help = "Examples:\n  scribe entry update <id> --author opus --add-topic api")]
    Update(UpdateArgs),
    /// Retire an agent note with a reason
    #[command(
        after_help = "Examples:\n  scribe entry retire <id> --author opus --reason 'No longer applies'"
    )]
    Retire(RetireArgs),
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

    /// Only entries carrying this topic; repeat it to require every one [default: any]
    #[arg(long, value_name = "TOPIC")]
    pub(crate) topic: Vec<Topic>,

    /// Only entries mentioning this reference (task:<id>, pr:<owner>/<repo>#<n>, repo:<slug> or session:<id>); repeat it to accept any of them [default: any]
    #[arg(long, value_name = "REF")]
    pub(crate) related: Vec<Related>,

    #[command(flatten)]
    pub(crate) dates: DateArgs,

    /// Only entries written under this author name, such as daniel or an agent's --author [default: any]
    #[arg(long, value_name = "NAME")]
    pub(crate) author: Option<AuthorName>,

    /// Only entries written through this channel: daniel (a #scribe line) or agent (entry add) [default: any]
    #[arg(long, value_name = "KIND", value_enum)]
    pub(crate) author_kind: Option<EntryAuthorKind>,

    /// Show at most this many of the matching entries, 1 or more; --json reports the total and whether the list was cut [default: all]
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<NonZeroUsize>,
}

/// The channels an entry can be written through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum EntryAuthorKind {
    // A #scribe line Daniel typed.
    Daniel,
    // A CLI write verb.
    Agent,
}

impl From<EntryAuthorKind> for AuthorKind {
    fn from(kind: EntryAuthorKind) -> Self {
        match kind {
            EntryAuthorKind::Daniel => Self::Daniel,
            EntryAuthorKind::Agent => Self::Agent,
        }
    }
}

/// Arguments to `entry show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The entry's id, as printed by `entry list`, or a unique prefix of it (at least four characters)
    #[arg(value_name = "ID")]
    pub(crate) id: String,
}

/// Allowed statuses come from the domain type, so help and parsing cannot
/// drift from it (STD-01 R25).
fn status_parser() -> impl TypedValueParser<Value = EntryStatus> {
    PossibleValuesParser::new(EntryStatus::NAMES).try_map(|s| s.parse::<EntryStatus>())
}

impl EntryCommand {
    /// Audit every write; list and show remain read-only.
    /// See [`super::Command::audit`].
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) | Self::Show(_) => None,
            Self::Add(_) => Some(AuditMeta {
                command: "entry add",
            }),
            Self::Update(_) => Some(AuditMeta {
                command: "entry update",
            }),
            Self::Retire(_) => Some(AuditMeta {
                command: "entry retire",
            }),
        }
    }
}

/// Run one `entry` verb.
pub(super) fn run(command: EntryCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        EntryCommand::List(args) => list(args, ctx),
        EntryCommand::Show(args) => show(&args, ctx),
        EntryCommand::Add(args) => {
            // Validate arguments before the redactor, so a bad tag or empty
            // content is reported before a `[redact] env` problem.
            let topics = scribe_core::Topics::try_from(args.topic.clone())
                .map_err(|e| CliError::Usage(e.to_string()))?;
            if args.content.trim().is_empty() {
                return Err(agent_refusal(
                    scribe_core::WriteRefusal::InvalidText,
                    "text must not be empty",
                ));
            }
            let config = ctx.store().config()?;
            if config.tags.resolve(args.tag.as_str()).is_none() {
                return Err(agent_refusal(
                    scribe_core::WriteRefusal::TagNotFound,
                    "unknown tag; run 'scribe tag list'",
                ));
            }
            let redactor = ctx.redactor(&config)?;
            let entry = ctx.store().add_agent_entry(
                scribe_core::AgentEntry {
                    tag: args.tag,
                    content: args.content,
                    project: args.project,
                    topics,
                    related: args.related,
                    host: ctx.host(),
                },
                args.author,
                ctx.now(),
                &redactor,
            )?;
            Ok(written(entry, true))
        }
        EntryCommand::Update(args) => {
            if args.content.as_deref().is_some_and(|c| c.trim().is_empty()) {
                return Err(agent_refusal(
                    scribe_core::WriteRefusal::InvalidText,
                    "text must not be empty",
                ));
            }
            let existing = ctx.store().resolve_entry(&args.id)?;
            if args.content.is_some() && existing.author.kind != AuthorKind::Agent {
                return Err(agent_refusal(
                    scribe_core::WriteRefusal::DanielOnly,
                    "Daniel's content changes only through #scribe",
                ));
            }
            let redactor = ctx.redactor(&ctx.store().config()?)?;
            let (entry, changed) = ctx.store().update_agent_entry(
                &args.id,
                scribe_core::EntryPatch {
                    content: args.content,
                    add_topics: args.add_topic,
                    remove_topics: args.remove_topic,
                    add_related: args.add_related,
                    remove_related: args.remove_related,
                },
                args.author,
                ctx.now(),
                &redactor,
            )?;
            Ok(written(entry, changed))
        }
        EntryCommand::Retire(args) => {
            check_reason(&args.reason)?;
            ctx.store().resolve_entry(&args.id)?;
            let redactor = ctx.redactor(&ctx.store().config()?)?;
            let (entry, changed) = ctx.store().retire_agent_entry(
                &args.id,
                &args.reason,
                args.author,
                ctx.now(),
                &redactor,
            )?;
            Ok(written(entry, changed))
        }
    }
}

fn list(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let filter = EntryFilter {
        tag: args.tag,
        status: args.status,
        project: args.project,
        topics: args.topic,
        related: args.related,
        dates: args.dates.range()?,
        author: args.author,
        author_kind: args.author_kind.map(AuthorKind::from),
        limit: args.limit,
    };
    let found = ctx.store().list_entries(&filter)?;
    let empty = format!(
        "no entries in {}; they are recorded by a #scribe line in a Claude Code session",
        ctx.store().root().display()
    );
    let mut criteria = criteria(&filter);
    criteria.extend(args.dates.criteria());
    let notice = ListNotice {
        noun: "entries",
        criteria,
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

/// The filters applied, as the flags that set them (the dates aside, which
/// the caller adds from its flags).
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
    for topic in &filter.topics {
        criteria.push(format!("--topic {topic}"));
    }
    for related in &filter.related {
        criteria.push(format!("--related {related}"));
    }
    if let Some(author) = &filter.author {
        criteria.push(format!("--author {author}"));
    }
    if let Some(kind) = filter.author_kind {
        criteria.push(format!("--author-kind {}", kind.as_str()));
    }
    criteria
}

fn show(args: &ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    let entry = ctx.store().resolve_entry(&args.id)?;
    Ok(Output::Show {
        record: Record::Entry(Box::new(EntryPayload::from(&entry))),
    })
}

#[derive(Debug, Args)]
pub(crate) struct AddArgs {
    /// Content to record
    content: String,
    /// Tag name or alias
    #[arg(long)]
    tag: TagName,
    /// Agent name (even daniel records the agent channel)
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Project slug
    #[arg(long, default_value = "global")]
    project: Project,
    /// Topic, repeatable (at most five)
    #[arg(long)]
    topic: Vec<Topic>,
    /// Related reference, repeatable
    #[arg(long)]
    related: Vec<scribe_core::Related>,
}

#[derive(Debug, Args)]
pub(crate) struct UpdateArgs {
    /// Entry id or unique prefix (at least four characters)
    id: String,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Replacement content
    #[arg(long)]
    content: Option<String>,
    /// Topic to add, repeatable
    #[arg(long)]
    add_topic: Vec<Topic>,
    /// Topic to remove, repeatable
    #[arg(long)]
    remove_topic: Vec<Topic>,
    /// Reference to add, repeatable
    #[arg(long)]
    add_related: Vec<scribe_core::Related>,
    /// Reference to remove, repeatable
    #[arg(long)]
    remove_related: Vec<scribe_core::Related>,
}

#[derive(Debug, Args)]
pub(crate) struct RetireArgs {
    /// Entry id or unique prefix
    id: String,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Why the note no longer applies
    #[arg(long)]
    reason: String,
}

pub(super) fn agent_refusal(kind: scribe_core::WriteRefusal, message: &str) -> CliError {
    CliError::Core(scribe_core::Error::AgentWrite {
        kind,
        message: message.to_owned(),
    })
}

pub(super) fn check_reason(reason: &str) -> Result<(), CliError> {
    if reason.trim().is_empty() {
        return Err(agent_refusal(
            scribe_core::WriteRefusal::InvalidText,
            "text must not be empty",
        ));
    }
    if reason.chars().count() > 300 {
        return Err(agent_refusal(
            scribe_core::WriteRefusal::InvalidText,
            "reason must contain at most 300 characters",
        ));
    }
    Ok(())
}

fn written(entry: scribe_core::Entry, changed: bool) -> Output {
    Output::written(Record::Entry(Box::new(EntryPayload::from(&entry))), changed)
}
