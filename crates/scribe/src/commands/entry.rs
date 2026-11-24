//! Entry listing, display and audited direct agent writes.

mod records;

use self::records::RecordArgs;
use super::{DateArgs, ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{EntryPayload, Output, Record, Records};
use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Args, Subcommand};
use scribe_core::EntryVerbs;
use scribe_core::cases::entry_cmd;
use scribe_core::{
    AuthorKind, AuthorName, EntryFilter, EntryId, EntryStatus, ItemState, Project, Related,
    TagName, Topic,
};
use std::num::NonZeroUsize;

pub(super) const ENTRY_EXAMPLES: &str = "\
Examples:
  scribe entry list --tag preferences
  scribe entry list --status active --project scribe
  scribe entry show <id>";

const LIST_EXAMPLES: &str = "\
Examples:
  scribe entry list
  scribe entry list --tag preferences --status active
  scribe entry list --topic ci --limit 10
  scribe entry list --since 2026-10-01 --until 2026-10-31 --author-kind user
  scribe entry list --related task:<id> --related pr:<owner>/<repo>#<n>
  scribe entry list --tag standard --after 2026-11-15T09:00:00Z --window --json
  scribe entry list --tag open-item --state open --state answered
  scribe entry list --json | jq '.entries[].content'
  scribe entry list | cut -f1,6";

const SHOW_EXAMPLES: &str = "\
Examples:
  scribe entry show <id>
  scribe entry show <id> --window
  scribe entry show <id> --json";

const ADD_EXAMPLES: &str = "\
Examples:
  scribe entry add --tag preferences --author opus 'Use the stable API'
  scribe entry add --tag preferences --author opus --related session:<id> --quote '<their words>' --spoken 'The user prefers short updates.'
  scribe entry add --tag idea --author opus --related session:<id> 'Cache the manifest between runs'
  scribe entry add --tag standard --author opus --related session:<id> --quote '<quoted words>' 'Name the unit in every size'
  scribe entry add --tag decision --author opus --decided-by user --quote '<their words>' --related session:<id> --related task:<id> 'Keep the stable API'
  scribe entry add --tag open-item --author opus --asked-by 0-chief --blocked 'Local checks' --answer-in hand 'Restart the dashboard'";

const RETIRE_EXAMPLES: &str = "\
The user's entry retires only on their instruction: --session and --instruction.

Examples:
  scribe entry retire <id> --author opus --reason 'No longer applies'
  scribe entry retire <id> --author opus --reason 'The user asked' --session <id> --instruction '<their words>'";

const FORGET_EXAMPLES: &str = "\
Blanks the entry's text, evidence and reason for good, with every proposal
quote of it and every reason about it; the id, tag, dates and author stay,
and no file is removed. It cannot be undone, so it runs only on the user's
instruction (their own turn says forget, erase, purge, permanently or for good)
and with --confirm.

Examples:
  scribe entry forget <id> --author opus --session <id> --instruction '<their words>' --confirm";

const ANSWER_EXAMPLES: &str = "\
The quote is checked as a decision's is: verbatim in a typed turn of the user's
in a named session on this host; a quote under eight characters must be the
whole turn. A later answer replaces an earlier one until the item closes.

Examples:
  scribe entry answer <id> --author opus --quote '<their words>' --related session:<id>";

const CLOSE_EXAMPLES: &str = "\
The asker closes an item once it has carried the answer out. A choice closes
only once answered; an action item closes answered or not.

Examples:
  scribe entry close <id> --author opus --decision <id>
  scribe entry close <id> --author opus --note 'Restarted; nothing lasting'";

const PICKUP_EXAMPLES: &str = "\
Examples:
  scribe entry pickup <id> --author standards-intake --result task:<id>
  scribe entry pickup <id> --author standards-intake --rejected";

/// Verbs on the `entry` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum EntryCommand {
    /// Add an agent note, decision or open item
    #[command(after_help = ADD_EXAMPLES)]
    Add(Box<AddArgs>),
    /// Update an entry's content or metadata
    ///
    /// Updates content or metadata; user entries and approved claims allow metadata only.
    #[command(after_help = "Examples:\n  scribe entry update <id> --author opus --add-topic api")]
    Update(UpdateArgs),
    /// Retire an entry with a reason
    ///
    /// Retires an agent note, or the user's entry on their instruction.
    #[command(after_help = RETIRE_EXAMPLES)]
    Retire(RetireArgs),
    /// Forget an entry for good on the user's instruction
    ///
    /// Blanks the entry's text and keeps its id.
    #[command(after_help = FORGET_EXAMPLES)]
    Forget(ForgetArgs),
    /// List entries, oldest first
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
    /// Show one entry in full: what was said, its content and its cite
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
    /// Record pickup or rejection of an action entry
    ///
    /// Records that a consumer picked up an action entry, or that its result was rejected.
    #[command(after_help = PICKUP_EXAMPLES)]
    Pickup(PickupArgs),
    /// Record the user's checked answer to an open item
    #[command(after_help = ANSWER_EXAMPLES)]
    Answer(AnswerArgs),
    /// Close an open item once carried out, with its decision or a note
    #[command(after_help = CLOSE_EXAMPLES)]
    Close(CloseArgs),
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

    /// Only entries of member sessions in this group (id, unique prefix, or project group slug)
    #[arg(long, value_name = "ID")]
    pub(crate) group: Option<String>,

    /// Only entries carrying this topic; repeat it to require every one [default: any]
    #[arg(long, value_name = "TOPIC")]
    pub(crate) topic: Vec<Topic>,

    /// Only entries mentioning this reference (task:<id>, pr:<owner>/<repo>#<n>, repo:<slug>, doc:<repo>/<path>#<anchor>, or session:<id>, whose id may be a unique prefix); repeat it to accept any of them [default: any]
    #[arg(long, value_name = "REF")]
    pub(crate) related: Vec<Related>,

    #[command(flatten)]
    pub(crate) dates: DateArgs,

    /// Only entries created strictly after this instant (RFC 3339, such as 2026-11-15T09:00:00Z): an intake's cursor [default: any]
    #[arg(long, value_name = "TIME", value_parser = parse_instant)]
    pub(crate) after: Option<time::OffsetDateTime>,

    /// Include each entry's transcript window (an action entry's chat before its #scribe line) [default: left out]
    #[arg(long)]
    pub(crate) window: bool,

    /// Only entries written under this author name, such as user or an agent's --author [default: any]
    #[arg(long, value_name = "NAME")]
    pub(crate) author: Option<AuthorName>,

    /// Only entries written through this channel: user (a #scribe line or an approval), agent (entry add) or scribe (an idea the session pass captured) [default: any]
    #[arg(long, value_name = "KIND", value_enum)]
    pub(crate) author_kind: Option<EntryAuthorKind>,

    /// Only open items in this derived state; repeat it to accept any of them [default: any entry]
    #[arg(long, value_name = "STATE", value_parser = state_parser())]
    pub(crate) state: Vec<ItemState>,

    /// Show at most this many of the matching entries, 1 or more; --json reports the total and whether the list was cut [default: all]
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<NonZeroUsize>,
}

/// The channels an entry can be written through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum EntryAuthorKind {
    // A #scribe line the user typed.
    #[value(alias = "daniel")]
    User,
    // A CLI write verb.
    Agent,
    // The session pass, capturing an idea itself.
    Scribe,
}

impl From<EntryAuthorKind> for AuthorKind {
    fn from(kind: EntryAuthorKind) -> Self {
        match kind {
            EntryAuthorKind::User => Self::User,
            EntryAuthorKind::Agent => Self::Agent,
            EntryAuthorKind::Scribe => Self::Scribe,
        }
    }
}

/// Arguments to `entry show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The entry's id, as printed by `entry list`, or a unique prefix of it (at least four characters)
    #[arg(value_name = "ID")]
    pub(crate) id: String,

    /// Include the entry's transcript window in full [default: only its size]
    #[arg(long)]
    pub(crate) window: bool,
}

/// An RFC 3339 instant.
fn parse_instant(text: &str) -> Result<time::OffsetDateTime, String> {
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .map_err(|_| format!("'{text}' is not an RFC 3339 time, such as 2026-11-15T09:00:00Z"))
}

/// Allowed statuses come from the domain type, so help and parsing cannot
/// drift from it (STD-01 R25).
fn status_parser() -> impl TypedValueParser<Value = EntryStatus> {
    PossibleValuesParser::new(EntryStatus::NAMES).try_map(|s| s.parse::<EntryStatus>())
}

/// Allowed open-item states come from the domain type (STD-01 R25).
fn state_parser() -> impl TypedValueParser<Value = ItemState> {
    PossibleValuesParser::new(ItemState::NAMES).try_map(|s| s.parse::<ItemState>())
}

impl EntryCommand {
    /// The entry and agent a write verb names, resolved before it runs, so a
    /// refusal is attributable. `add` names its target only in its output.
    pub(super) fn subject(&self, ctx: &Context) -> AuditSubject {
        match self {
            Self::List(_) | Self::Show(_) => AuditSubject::default(),
            Self::Add(args) => AuditSubject {
                target: None,
                author: Some(args.author.as_str().to_owned()),
            },
            Self::Update(args) => AuditSubject {
                target: resolved_entry(ctx, &args.id),
                author: Some(args.author.as_str().to_owned()),
            },
            Self::Retire(args) => AuditSubject {
                target: resolved_entry(ctx, &args.id),
                author: Some(args.author.as_str().to_owned()),
            },
            Self::Forget(args) => AuditSubject {
                target: resolved_entry(ctx, &args.id),
                author: Some(args.author.as_str().to_owned()),
            },
            Self::Pickup(args) => AuditSubject {
                target: resolved_entry(ctx, &args.id),
                author: Some(args.author.as_str().to_owned()),
            },
            Self::Answer(args) => AuditSubject {
                target: resolved_entry(ctx, &args.id),
                author: Some(args.author.as_str().to_owned()),
            },
            Self::Close(args) => AuditSubject {
                target: resolved_entry(ctx, &args.id),
                author: Some(args.author.as_str().to_owned()),
            },
        }
    }

    /// Audit every write; list and show remain read-only.
    /// See [`super::Command::audit`].
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) | Self::Show(_) => None,
            Self::Add(_) => Some(AuditMeta {
                command: "entry add",
                stepwise: false,
            }),
            Self::Update(_) => Some(AuditMeta {
                command: "entry update",
                stepwise: false,
            }),
            Self::Retire(_) => Some(AuditMeta {
                command: "entry retire",
                stepwise: false,
            }),
            Self::Forget(_) => Some(AuditMeta {
                command: "entry forget",
                stepwise: false,
            }),
            Self::Pickup(_) => Some(AuditMeta {
                command: "entry pickup",
                stepwise: false,
            }),
            Self::Answer(_) => Some(AuditMeta {
                command: "entry answer",
                stepwise: false,
            }),
            Self::Close(_) => Some(AuditMeta {
                command: "entry close",
                stepwise: false,
            }),
        }
    }
}

/// The id of the entry `input` resolves to, or `None` when it does not.
fn resolved_entry(ctx: &Context, input: &str) -> Option<String> {
    ctx.store()
        .resolve_entry(input)
        .ok()
        .map(|entry| entry.id.to_string())
}

/// Run one `entry` verb.
pub(super) fn run(command: EntryCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        EntryCommand::List(args) => list(args, ctx),
        EntryCommand::Show(args) => show(&args, ctx),
        EntryCommand::Add(args) => {
            let written = entry_cmd::add(
                &ctx.core(),
                entry_cmd::AddInput {
                    tag: args.tag,
                    content: args.content,
                    author: args.author,
                    project: args.project,
                    topics: args.topic,
                    related: args.related,
                    quote: args.quote,
                    mode: args.mode,
                    evidence: args.evidence,
                    as_of: args.as_of,
                    spoken: args.spoken,
                    record: args.record.into_fields(),
                    provider: ctx.provider(),
                    host: ctx.host(),
                    current_session: std::env::var("CLAUDE_CODE_SESSION_ID").ok(),
                },
            )?;
            Ok(output(written))
        }
        EntryCommand::Pickup(args) => {
            let written = entry_cmd::pickup(
                &ctx.core(),
                entry_cmd::PickupInput {
                    id: &args.id,
                    author: args.author,
                    result: args.result.as_ref(),
                    rejected: args.rejected,
                    provider: ctx.provider(),
                },
            )?;
            Ok(output(written))
        }
        EntryCommand::Update(args) => {
            let patch = scribe_core::EntryPatch {
                content: args.content,
                decision_options: (args.clear_options || !args.options.is_empty())
                    .then_some(args.options),
                decision_recommended: args.recommended,
                decision_rationale: args.rationale,
                add_topics: args.add_topic,
                remove_topics: args.remove_topic,
                add_related: args.add_related,
                remove_related: args.remove_related,
            };
            let written =
                entry_cmd::update(&ctx.core(), &args.id, patch, args.author, ctx.provider())?;
            Ok(output(written))
        }
        EntryCommand::Retire(args) => {
            let written = entry_cmd::retire(
                &ctx.core(),
                &args.id,
                &args.reason,
                args.author,
                &args.instruction.case(ctx),
            )?;
            Ok(output(written))
        }
        EntryCommand::Forget(args) => {
            let written = entry_cmd::forget(
                &ctx.core(),
                &args.id,
                &args.author,
                args.confirm,
                &args.instruction.case(ctx),
            )?;
            Ok(output(written))
        }
        EntryCommand::Answer(args) => {
            let written = entry_cmd::answer(
                &ctx.core(),
                &args.id,
                &args.author,
                &args.quote,
                &args.related,
                ctx.provider(),
            )?;
            Ok(output(written))
        }
        EntryCommand::Close(args) => {
            let written = entry_cmd::close(
                &ctx.core(),
                &args.id,
                args.decision.as_ref(),
                args.note.as_deref(),
                args.author,
            )?;
            Ok(output(written))
        }
    }
}

/// An entry write's result as output, with its notices.
fn output(written: entry_cmd::EntryWritten) -> Output {
    let payload = EntryPayload::from(&written.entry).with_action(written.action.as_ref());
    let mut output = Output::written(Record::Entry(Box::new(payload)), written.changed);
    if let Output::Written { notices, .. } = &mut output {
        notices.extend(written.notices);
    }
    output
}

fn list(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let listed = scribe_core::cases::entry::list(
        &ctx.core(),
        scribe_core::cases::entry::ListInput {
            filter: EntryFilter {
                tag: args.tag,
                status: args.status,
                project: args.project,
                topics: args.topic,
                related: args.related.clone(),
                dates: scribe_core::DateRange::default(),
                after: args.after,
                author: args.author.map(scribe_core::AuthorName::current),
                author_kind: args.author_kind.map(AuthorKind::from),
                states: args.state,
                limit: args.limit,
            },
            since: args.dates.since,
            until: args.dates.until,
            group: args.group,
            provider: ctx.provider(),
        },
    )?;
    let (filter, group, found) = (listed.filter, listed.group, listed.found);
    let empty = format!(
        "no entries in {}; they are recorded by a #scribe line in a Claude Code session",
        ctx.store().root().display()
    );
    let mut criteria = criteria_with_related(&filter, &args.related);
    if let Some(group) = group {
        criteria.push(format!("--group {}", group.id));
    }
    criteria.extend(args.dates.criteria());
    if let Some(after) = args.after
        && let Ok(text) = after.format(&time::format_description::well_known::Rfc3339)
    {
        criteria.push(format!("--after {text}"));
    }
    let notice = ListNotice {
        noun: "entries",
        criteria,
        empty: &empty,
    };
    let actions = Actions(listed.actions);
    let records = Records::Entries(
        found
            .listing
            .items
            .iter()
            .map(|e| actions.payload(e, args.window))
            .collect(),
    );
    Ok(list_output(
        records,
        found.listing.total,
        &found.skipped,
        &notice,
    ))
}

/// The filters applied, as the flags that set them (the dates aside, which
/// the caller adds from its flags).
#[cfg(test)]
pub(super) fn criteria(filter: &EntryFilter) -> Vec<String> {
    criteria_with_related(filter, &filter.related)
}

fn criteria_with_related(filter: &EntryFilter, related: &[scribe_core::Related]) -> Vec<String> {
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
    for related in related {
        criteria.push(format!("--related {related}"));
    }
    if let Some(author) = &filter.author {
        criteria.push(format!("--author {author}"));
    }
    if let Some(kind) = filter.author_kind {
        criteria.push(format!("--author-kind {}", kind.as_str()));
    }
    for state in &filter.states {
        criteria.push(format!("--state {state}"));
    }
    criteria
}

fn show(args: &ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    let shown = scribe_core::cases::entry::show(&ctx.core(), &args.id)?;
    let actions = Actions(shown.actions);
    Ok(Output::Show {
        record: Record::Entry(Box::new(
            actions
                .payload(&shown.entry, args.window)
                .with_admitted(shown.admitted),
        )),
    })
}

struct Actions(entry_cmd::Actions);

impl Actions {
    /// `entry` as output, with its action state when its tag is an action
    /// tag and its window when `window`.
    fn payload(&self, entry: &scribe_core::Entry, window: bool) -> EntryPayload {
        let state = self.0.state(entry);
        let payload = EntryPayload::from(entry).with_action(state.as_ref());
        if window {
            payload.with_window(entry)
        } else {
            payload
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct AddArgs {
    /// Content to record
    content: String,
    /// Tag name or alias
    #[arg(long)]
    tag: TagName,
    /// Agent name (always recorded as the agent channel; reserved names are refused)
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Project slug
    #[arg(long, default_value = "global")]
    project: Project,
    /// Topic, repeatable (at most five)
    #[arg(long)]
    topic: Vec<Topic>,
    /// Related reference, repeatable (task:, pr:, repo:, doc:, session:); name the source session for checked quotes or the writing session for action tags
    #[arg(long)]
    related: Vec<scribe_core::Related>,
    /// The user's verbatim words from a related session; required for a user decision and by quote tags except a verified self-refresh handoff
    #[arg(long)]
    quote: Option<String>,
    /// Handoff refresh mode: continue or new
    #[arg(long)]
    mode: Option<scribe_core::handoff::Mode>,
    /// Evidence pointer (a path, task, run, experiment or commit); required under a require_evidence tag, refused under any other tag
    #[arg(long, value_name = "POINTER")]
    evidence: Option<String>,
    /// The date a require_evidence entry's result was true, as YYYY-MM-DD [default: today]
    #[arg(long, value_name = "DATE", value_parser = scribe_core::parse_day)]
    as_of: Option<time::Date>,
    /// Record an ordinary memory note on the user's spoken instruction; sets stated_by to --author and requires --quote and --related session:<id>
    #[arg(long)]
    spoken: bool,
    #[command(flatten)]
    record: RecordArgs,
}

#[derive(Debug, Args)]
pub(crate) struct PickupArgs {
    /// The action entry's id or a unique prefix (at least four characters)
    id: String,
    /// The consumer's name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// What the consumer made of it (task:<id>, pr:<owner>/<repo>#<n>, repo:<slug> or session:<id>, whose id may be a unique prefix)
    #[arg(long, value_name = "REF")]
    result: Option<scribe_core::Related>,
    /// The user rejected the result
    #[arg(long)]
    rejected: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AnswerArgs {
    /// The open item's id or a unique prefix (at least four characters)
    id: String,
    /// Agent name of the session recording the answer
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// The user's answer, verbatim from a typed turn of theirs in a related session
    #[arg(long)]
    quote: String,
    /// The session the user typed the answer in, as session:<id> (a unique prefix is accepted); repeatable
    #[arg(long, value_name = "REF", required = true)]
    related: Vec<scribe_core::Related>,
}

#[derive(Debug, Args)]
#[command(group = clap::ArgGroup::new("settled").required(true).multiple(true).args(["decision", "note"]))]
pub(crate) struct CloseArgs {
    /// The open item's id or a unique prefix (at least four characters)
    id: String,
    /// Agent name of the asker closing it
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// The active decision written for this item with --settles
    #[arg(long, value_name = "ID")]
    decision: Option<EntryId>,
    /// What was done, when no lasting decision came of it (at most 300 characters)
    #[arg(long)]
    note: Option<String>,
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
    /// Replace a decision's options in full, repeatable
    #[arg(long = "option")]
    options: Vec<String>,
    /// Clear a decision's options and recommendation
    #[arg(long, conflicts_with_all = ["options", "recommended"])]
    clear_options: bool,
    /// Set the 1-based recommendation for a decision
    #[arg(long)]
    recommended: Option<usize>,
    /// Replace an agent decision's rationale
    #[arg(long)]
    rationale: Option<String>,
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
    #[command(flatten)]
    instruction: super::instruction::InstructionArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ForgetArgs {
    /// Entry id or unique prefix (at least four characters)
    id: String,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Confirm blanking the entry's text for good; it cannot be undone
    #[arg(long)]
    confirm: bool,
    #[command(flatten)]
    instruction: super::instruction::InstructionArgs,
}
