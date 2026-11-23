//! Entry listing, display and audited direct agent writes.

use super::{DateArgs, ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
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
  scribe entry add --tag standard --author opus --related session:<id> --quote '<quoted words>' 'Name the unit in every size'";

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

const PICKUP_EXAMPLES: &str = "\
Examples:
  scribe entry pickup <id> --author standards-intake --result task:<id>
  scribe entry pickup <id> --author standards-intake --rejected";

/// Verbs on the `entry` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum EntryCommand {
    /// Add an agent note (never attributed to the user)
    #[command(after_help = ADD_EXAMPLES)]
    Add(AddArgs),
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

    /// Only entries mentioning this reference (task:<id>, pr:<owner>/<repo>#<n>, repo:<slug> or session:<id>, whose id may be a unique prefix); repeat it to accept any of them [default: any]
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
            scribe_core::check_agent_author(&args.author)?;
            // Arguments first: a bad tag or empty content is reported before
            // an unset [redact] env name.
            let topics = scribe_core::Topics::try_from(args.topic)
                .map_err(|e| CliError::Usage(e.to_string()))?;
            // A session given as a unique prefix is stored as the full id.
            let related = super::resolve_relateds(ctx, &args.related)?;
            let def = ctx.store().check_new_entry_full(
                &args.tag,
                &args.content,
                args.quote.as_deref(),
                args.evidence.as_deref(),
                args.spoken,
            )?;
            scribe_core::check_handoff_related(&def.name, &related)?;
            if let Some(quote) = &args.quote {
                verify_quote(
                    ctx,
                    &related,
                    quote,
                    def.name.as_str() == scribe_core::HANDOFF_TAG,
                )?;
            }
            let self_refresh_verified =
                if def.name.as_str() == scribe_core::HANDOFF_TAG && args.quote.is_none() {
                    verify_self_refresh(ctx, &related, args.mode)?;
                    true
                } else {
                    false
                };
            let redactor = ctx.redactor(&ctx.store().config()?)?;
            let entry = ctx.store().add_agent_entry(
                scribe_core::AgentEntry {
                    tag: args.tag,
                    content: args.content,
                    project: args.project,
                    topics,
                    related,
                    host: ctx.host(),
                    quote: args.quote,
                    evidence: args.evidence,
                    as_of: args.as_of,
                    spoken: args.spoken,
                    handoff_mode: args.mode,
                    self_refresh_verified,
                },
                args.author,
                ctx.now(),
                &redactor,
            )?;
            written_with_action(ctx, entry, true)
        }
        EntryCommand::Pickup(args) => {
            scribe_core::check_agent_author(&args.author)?;
            let result = args
                .result
                .as_ref()
                .map(|r| super::resolve_related(ctx, r))
                .transpose()?;
            let (pickup, changed) = ctx.store().record_pickup(
                &args.id,
                args.author,
                result,
                args.rejected,
                ctx.now(),
            )?;
            let entry = ctx.store().entry(&pickup.entry)?;
            written_with_action(ctx, entry, changed)
        }
        EntryCommand::Update(args) => {
            scribe_core::check_agent_author(&args.author)?;
            let patch = scribe_core::EntryPatch {
                content: args.content,
                add_topics: args.add_topic,
                remove_topics: args.remove_topic,
                add_related: super::resolve_relateds(ctx, &args.add_related)?,
                remove_related: resolve_relateds_preserving_input(ctx, &args.remove_related)?,
            };
            ctx.store().check_agent_entry_update(&args.id, &patch)?;
            let redactor = ctx.redactor(&ctx.store().config()?)?;
            let (entry, changed) = ctx.store().update_agent_entry(
                &args.id,
                patch,
                args.author,
                ctx.now(),
                &redactor,
            )?;
            written_with_action(ctx, entry, changed)
        }
        EntryCommand::Retire(args) => {
            args.instruction.note(ctx);
            scribe_core::check_agent_author(&args.author)?;
            scribe_core::check_reason(&args.reason)?;
            let target = ctx.store().resolve_entry(&args.id)?;
            // Before the store lock; a refusal changes nothing. A passing
            // check lets retire accept Daniel's entry (§12.1).
            let instructed = super::instruction::check(
                ctx,
                &args.instruction,
                scribe_core::instruction::Act::Retire,
                &scribe_core::instruction::Target::new(target.id.to_string(), &args.id),
            )?;
            let redactor = ctx.redactor(&ctx.store().config()?)?;
            let (entry, changed) = ctx.store().retire_agent_entry(
                &args.id,
                &args.reason,
                instructed.as_ref(),
                args.author,
                ctx.now(),
                &redactor,
            )?;
            written_with_action(ctx, entry, changed)
        }
        EntryCommand::Forget(args) => forget(&args, ctx),
    }
}

/// `entry forget`: exists only with an instruction and `--confirm`; both
/// refusals, and every check of the instruction, come before the store lock
/// and change nothing (§12.4).
fn forget(args: &ForgetArgs, ctx: &Context) -> Result<Output, CliError> {
    args.instruction.note(ctx);
    scribe_core::check_agent_author(&args.author)?;
    let target = ctx.store().resolve_entry(&args.id)?;
    let uninstructed = || {
        CliError::Usage(String::from(
            "entry forget runs only on the user's instruction: pass --session <id> and --instruction '<their words>'",
        ))
    };
    if args.instruction.session.is_none() || args.instruction.instruction.is_none() {
        return Err(uninstructed());
    }
    if !args.confirm {
        return Err(CliError::ForgetConfirmRequired(target.id.to_string()));
    }
    let Some(instructed) = super::instruction::check(
        ctx,
        &args.instruction,
        scribe_core::instruction::Act::Forget,
        &scribe_core::instruction::Target::new(target.id.to_string(), &args.id),
    )?
    else {
        return Err(uninstructed());
    };
    let (entry, changed) =
        ctx.store()
            .forget_entry(&args.id, &instructed, args.author.clone(), ctx.now())?;
    written_with_action(ctx, entry, changed)
}

fn list(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let filter = EntryFilter {
        tag: args.tag,
        status: args.status,
        project: args.project,
        topics: args.topic,
        related: resolve_relateds_preserving_input(ctx, &args.related)?,
        dates: args.dates.range()?,
        after: args.after,
        author: args.author.map(scribe_core::AuthorName::current),
        author_kind: args.author_kind.map(AuthorKind::from),
        limit: args.limit,
    };
    let store = ctx.store();
    let group = args
        .group
        .as_deref()
        .map(|id| store.group(id))
        .transpose()?;
    let members = group.as_ref().map(|group| group.members());
    let found = store.list_entries_scoped(&filter, members.as_ref())?;
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
    let actions = Actions::load(ctx)?;
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
    criteria
}

/// Resolve session prefixes for validation and canonical matching while
/// retaining each caller-supplied reference. Existing entries can contain
/// an unknown session reference literally, so lookup and removal must also
/// compare that original value after a matching session is cataloged.
fn resolve_relateds_preserving_input(
    ctx: &Context,
    related: &[scribe_core::Related],
) -> Result<Vec<scribe_core::Related>, CliError> {
    let resolved = super::resolve_relateds(ctx, related)?;
    let mut preserved = Vec::with_capacity(resolved.len() * 2);
    for (input, canonical) in related.iter().zip(resolved) {
        if !preserved.contains(input) {
            preserved.push(input.clone());
        }
        if !preserved.contains(&canonical) {
            preserved.push(canonical);
        }
    }
    Ok(preserved)
}

fn show(args: &ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    let entry = ctx.store().resolve_entry(&args.id)?;
    let actions = Actions::load(ctx)?;
    // An entry approved from a pass claim shows what admitted the claim:
    // the approved candidate proposal that reserved its id.
    let admitted = ctx
        .store()
        .proposals()?
        .into_iter()
        .find(|p| {
            p.kind == scribe_core::ProposalKind::Candidate
                && p.state == scribe_core::ProposalState::Approved
                && p.older == entry.id
        })
        .and_then(|p| p.candidate.and_then(|c| c.admitted));
    Ok(Output::Show {
        record: Record::Entry(Box::new(
            actions.payload(&entry, args.window).with_admitted(admitted),
        )),
    })
}

/// What an entry command needs to report action state: the tag table, the
/// pickups and the time.
struct Actions {
    tags: scribe_core::TagTable,
    pickups: Vec<scribe_core::action::Pickup>,
    now: time::OffsetDateTime,
}

impl Actions {
    fn load(ctx: &Context) -> Result<Self, CliError> {
        Ok(Self {
            tags: ctx.store().config()?.tags,
            pickups: ctx.store().pickups()?,
            now: ctx.now(),
        })
    }

    /// `entry` as output, with its action state when its tag is an action
    /// tag and its window when `window`.
    fn payload(&self, entry: &scribe_core::Entry, window: bool) -> EntryPayload {
        let state = self
            .tags
            .action(&entry.tag)
            .map(|_| scribe_core::action::state(entry, &self.pickups, self.now));
        let payload = EntryPayload::from(entry).with_action(state.as_ref());
        if window {
            payload.with_window(entry)
        } else {
            payload
        }
    }
}

/// Refuse a quote that is not verbatim in the user's typed turns of a session
/// the entry names (`--related session:<id>`), or whose transcript cannot be
/// read on this host (docs/design/scribe/2_design.md §4.6). When
/// `require_all_sessions` is true (used for `handoff`, §16.1), every
/// `session:` reference in `related` must parse as a `SessionId` and verify
/// the quote against its transcript.
fn verify_quote(
    ctx: &Context,
    related: &[Related],
    quote: &str,
    require_all_sessions: bool,
) -> Result<(), CliError> {
    use super::hook::payload;
    let refuse = |kind, message: String| -> CliError {
        scribe_core::Error::AgentWrite { kind, message }.into()
    };
    let session_refs: Vec<&Related> = related
        .iter()
        .filter(|r| r.kind() == scribe_core::RelatedKind::Session)
        .collect();
    let sessions: Vec<scribe_core::SessionId> = if require_all_sessions {
        let mut parsed = Vec::with_capacity(session_refs.len());
        for rel in &session_refs {
            let Ok(id) = rel.target().parse::<scribe_core::SessionId>() else {
                return Err(refuse(
                    scribe_core::WriteRefusal::QuoteNotFound,
                    format!("cannot verify quote for invalid session reference '{rel}'"),
                ));
            };
            if !parsed.contains(&id) {
                parsed.push(id);
            }
        }
        parsed
    } else {
        session_refs
            .iter()
            .filter_map(|r| r.target().parse().ok())
            .collect()
    };
    if sessions.is_empty() {
        return Err(refuse(
            scribe_core::WriteRefusal::QuoteRequired,
            String::from(
                "a quote is checked against the session it was typed in; pass --related session:<id>",
            ),
        ));
    }
    let check_one = |id: &scribe_core::SessionId| -> Result<bool, CliError> {
        let Some(path) = payload::locate(ctx, id)? else {
            return Ok(false);
        };
        if !matches!(
            payload::check(ctx, &path, id),
            Ok(payload::Transcript::Present)
        ) {
            return Ok(false);
        }
        match payload::read_for(ctx, &path) {
            Ok(bytes) => {
                let messages = scribe_core::transcript::parse(&bytes);
                Ok(scribe_core::action::quote_found(&messages, quote))
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "cannot read the transcript");
                Ok(false)
            }
        }
    };
    let not_found = || {
        refuse(
            scribe_core::WriteRefusal::QuoteNotFound,
            String::from(
                "the quote is not word for word in the user's typed turns of the session named, or its transcript is not readable on this host; quote the user's own words exactly, or file a friction instead",
            ),
        )
    };
    if require_all_sessions {
        for id in &sessions {
            if !check_one(id)? {
                return Err(not_found());
            }
        }
        return Ok(());
    }
    for id in &sessions {
        if check_one(id)? {
            return Ok(());
        }
    }
    Err(not_found())
}

fn verify_self_refresh(
    ctx: &Context,
    related: &[Related],
    mode: Option<scribe_core::handoff::Mode>,
) -> Result<(), CliError> {
    use super::hook::payload;
    let refuse = |kind, message: &str| -> CliError {
        scribe_core::Error::AgentWrite {
            kind,
            message: message.to_owned(),
        }
        .into()
    };
    let current = std::env::var("CLAUDE_CODE_SESSION_ID").ok();
    let target = related
        .iter()
        .find(|r| r.kind() == scribe_core::RelatedKind::Session);
    if mode.is_none() || current.as_deref() != target.map(Related::target) || current.is_none() {
        return Err(refuse(
            scribe_core::WriteRefusal::QuoteRequired,
            "a quote-less refresh requires --mode and the current CLAUDE_CODE_SESSION_ID matching the related session",
        ));
    }
    let id = target
        .and_then(|r| r.target().parse::<scribe_core::SessionId>().ok())
        .ok_or_else(|| {
            refuse(
                scribe_core::WriteRefusal::QuoteRequired,
                "invalid refresh session",
            )
        })?;
    // Walk launchers in metadata order with one budget and cycle guard. A
    // clear predecessor takes precedence over every launch edge at that node.
    let mut last_failure: CliError = scribe_core::Error::AgentWrite {
        kind: scribe_core::WriteRefusal::QuoteNotFound,
        message: format!("refresh session {id}: transcript is not readable on this host"),
    }
    .into();
    let mut pending = vec![id];
    let mut seen = std::collections::HashSet::new();
    while let Some(link) = pending.pop() {
        let failed = |reason: &str| -> CliError {
            scribe_core::Error::AgentWrite {
                kind: scribe_core::WriteRefusal::QuoteNotFound,
                message: format!("refresh session {link}: {reason}"),
            }
            .into()
        };
        if !seen.insert(link.clone()) {
            return Err(failed("ancestry contains a cycle"));
        }
        let path = payload::locate(ctx, &link).ok().flatten();
        let bytes = path
            .as_ref()
            .and_then(|path| payload::read_checked(ctx, path, &link).ok().flatten());
        if let Some(bytes) = bytes {
            let typed = if ctx.provider() == scribe_core::Provider::ClaudeCode {
                scribe_core::instruction::claude_conversation_raw_typed(&bytes)
                    .iter()
                    .any(|turn| turn.typed.is_some())
            } else {
                scribe_core::transcript::parse(&bytes)
                    .iter()
                    .filter_map(scribe_core::transcript::user_turn)
                    .any(|turn| !turn.own.trim().is_empty())
            };
            if typed {
                return Ok(());
            }
            let parents = if ctx.provider() == scribe_core::Provider::ClaudeCode {
                scribe_core::transcript::predecessors(&bytes)
            } else {
                scribe_core::transcript::Predecessors::default()
            };
            if parents.clear.len() > 1 {
                return Err(failed("ambiguous: more than one clear predecessor"));
            }
            if let Some(parent) = parents.clear.first() {
                pending.push(parent.clone());
            } else {
                pending.extend(parents.launch.into_iter().rev());
            }
            last_failure = failed(
                "transcript must contain a typed user turn or a clear or launch predecessor",
            );
        } else {
            last_failure = failed("transcript failed provenance or is not readable on this host");
        }
        if seen.len() == 16 && !pending.is_empty() {
            return Err(failed("ancestry exceeds the 16-transcript limit"));
        }
    }
    Err(last_failure)
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
    /// Related reference, repeatable; under an action tag, name the session you write in (session:<id>, whose id may be a unique prefix, stored in full)
    #[arg(long)]
    related: Vec<scribe_core::Related>,
    /// The user's words, verbatim from a typed turn in the related session; required by quote tags except a verified self-refresh handoff
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
    /// Record on the user's spoken instruction: sets stated_by to --author; requires --quote, --related session:<id>, and a non-action memory tag (not handoff or require_evidence)
    #[arg(long)]
    spoken: bool,
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

fn written_with_action(
    ctx: &Context,
    entry: scribe_core::Entry,
    changed: bool,
) -> Result<Output, CliError> {
    let payload = Actions::load(ctx)?.payload(&entry, false);
    Ok(Output::written(Record::Entry(Box::new(payload)), changed))
}
