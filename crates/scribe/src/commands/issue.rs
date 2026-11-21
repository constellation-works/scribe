//! The `issue` noun (docs/design/scribe/2_design.md §11.4): the memory of an
//! incident that recurs across projects, and what fixed it.
//!
//! The agent that answers Daniel's investigation request writes it: `add`
//! for a new issue, `occur` for another occurrence of one, `update` for the
//! header's cause, fix and status, and `move` for an occurrence filed under
//! the wrong issue. `add` and `occur` carry Daniel's request word for word
//! (`--request`), checked against his typed turns in the session named by
//! `--related session:<id>`; error lines are the agent's evidence and are
//! not checked. No write waits for approval: Daniel removes what is wrong
//! with `#scribe forget <issue-id>` or `<issue-id>#<n>`. `search`, `show` and
//! `list` read only.
//!
//! Every write is one audit line carrying the verb, the issue, the
//! occurrence, the session and the request's message uuid: ids only, never
//! the issue's words.

use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{HitPayload, Output, Record, Records};
use clap::{Args, Subcommand};
use scribe_core::ask::{Corpus, RecordKind};
use scribe_core::issue::{IssueFilter, IssuePatch, IssueStatus, NewIssue, NewOccurrence};
use scribe_core::search::{self, DEFAULT_LIMIT, SearchError, SearchQuery};
use scribe_core::{
    Author, AuthorName, DateRange, Label, MoveTo, Project, Related, RelatedKind, SessionId, Topic,
    Topics, WriteRefusal,
};
use std::num::NonZeroUsize;

pub(super) const ISSUE_EXAMPLES: &str = "\
Examples:
  scribe issue search 'HTTP 429 secondary rate limit'
  scribe issue list --status open
  scribe issue show <issue-id>
  scribe issue occur <issue-id> --author opus --related session:<id> --request '<his words>' --summary '<what happened>'";

const ADD_EXAMPLES: &str = "\
Search first (scribe issue search); add only when no issue matches, else
occur. --request is Daniel's request word for word from one of his typed
turns in the session --related session:<id> names. Error lines are your
evidence and are shown as such. Without a known cause, leave --cause out:
it reads \"cause not found\". Daniel cancels with #scribe forget <issue-id>.

Examples:
  scribe issue add --author opus --related session:<id> --request '<his words>' --title '<title>' --signature 'HTTP 429' --summary '<what happened>'
  scribe issue add --author opus --related session:<id> --related task:<id> --request '<his words>' --title '<title>' --signature 'HTTP 429' --signature 'secondary rate limit' --summary '<what happened>' --error-line '<a key error line>' --cause '<the cause>' --fix '<the fix>' --project orbit";

const OCCUR_EXAMPLES: &str = "\
An occurrence on a resolved issue reopens it and is marked \"recurred after
fix\". Daniel cancels with #scribe forget <issue-id>#<n>.

Examples:
  scribe issue occur <issue-id> --author opus --related session:<id> --request '<his words>' --summary '<what happened>'
  scribe issue occur <issue-id> --author opus --related session:<id> --request '<his words>' --summary '<what happened>' --error-line '<a key error line>' --fix '<what fixed it>'";

const UPDATE_EXAMPLES: &str = "\
Examples:
  scribe issue update <issue-id> --author opus --cause '<the cause found>' --fix '<the fix>' --status resolved
  scribe issue update <issue-id> --author opus --status mitigated --related session:<id> --request '<his words>'";

const MOVE_EXAMPLES: &str = "\
Moves a wrongly matched occurrence; it keeps where it came from (moved_from).
Its old number is never given again.

Examples:
  scribe issue move <issue-id>#<n> --author opus --to <issue-id>
  scribe issue move <issue-id>#<n> --author opus --new --title '<title>' --signature '<symptom>'";

const SEARCH_EXAMPLES: &str = "\
Ranks issues by their titles, signatures, causes, fixes and occurrences with
BM25, as record search --kind issue does, with no model call. Run it before
diagnosing.

Examples:
  scribe issue search 'HTTP 429 secondary rate limit'
  scribe issue search 'cpu exhausted' --project orbit --json | jq -r '.hits[].cite'";

const SHOW_EXAMPLES: &str = "\
Examples:
  scribe issue show <issue-id>
  scribe issue show <issue-id> --json";

const LIST_EXAMPLES: &str = "\
Examples:
  scribe issue list
  scribe issue list --status open --project orbit
  scribe issue list --json | jq -r '.issues[].title'";

/// Verbs on the `issue` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum IssueCommand {
    /// Record a new issue with its first occurrence (no approval; Daniel forgets what is wrong)
    #[command(after_help = ADD_EXAMPLES)]
    Add(AddArgs),
    /// Add an occurrence to an issue; one on a resolved issue reopens it
    #[command(after_help = OCCUR_EXAMPLES)]
    Occur(OccurArgs),
    /// Change an issue's cause, fix or status
    #[command(after_help = UPDATE_EXAMPLES)]
    Update(UpdateArgs),
    /// Move a wrongly matched occurrence to another issue or a new one
    #[command(after_help = MOVE_EXAMPLES)]
    Move(MoveArgs),
    /// Rank issues for words, best first, with no model call
    #[command(after_help = SEARCH_EXAMPLES)]
    Search(SearchArgs),
    /// Show one issue in full: its cause, fix and every occurrence
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
    /// List issues, most recently updated first
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
}

/// Daniel's request and the session it was typed in.
#[derive(Debug, Args)]
pub(crate) struct RequestArgs {
    /// Reference, repeatable: session:<id> names the session Daniel's request was typed in; task:<id>, pr:<owner>/<repo>#<n> and repo:<slug> are linked
    #[arg(long, value_name = "REF")]
    related: Vec<Related>,
    /// Daniel's request, word for word from one of his typed turns in the session --related names
    #[arg(long, value_name = "WORDS")]
    request: Option<String>,
}

/// What one occurrence records.
#[derive(Debug, Args)]
pub(crate) struct OccurrenceArgs {
    /// What happened, in your words (at most 1200 characters)
    #[arg(long)]
    summary: String,
    /// A key error line, repeatable (at most 10, 300 characters each); shown as your evidence
    #[arg(long, value_name = "LINE")]
    error_line: Vec<String>,
    /// The cause this occurrence found [default: cause not found]
    #[arg(long)]
    cause: Option<String>,
    /// The fix or mitigation it applied [default: fix not found]
    #[arg(long)]
    fix: Option<String>,
    /// The project it happened in (a repository slug, or global)
    #[arg(long, default_value = Project::GLOBAL)]
    project: Project,
}

#[derive(Debug, Args)]
pub(crate) struct AddArgs {
    /// Agent name
    #[arg(long)]
    author: AuthorName,
    /// The issue's title (at most 120 characters)
    #[arg(long)]
    title: String,
    /// A symptom or error string a later search should match, repeatable (1 to 5, 200 characters each)
    #[arg(long, value_name = "TEXT", required = true)]
    signature: Vec<String>,
    /// Topic, repeatable (at most five)
    #[arg(long)]
    topic: Vec<Topic>,
    #[command(flatten)]
    request: RequestArgs,
    #[command(flatten)]
    occurrence: OccurrenceArgs,
}

#[derive(Debug, Args)]
pub(crate) struct OccurArgs {
    /// Issue id or unique prefix (at least four characters)
    id: String,
    /// Agent name
    #[arg(long)]
    author: AuthorName,
    #[command(flatten)]
    request: RequestArgs,
    #[command(flatten)]
    occurrence: OccurrenceArgs,
}

#[derive(Debug, Args)]
pub(crate) struct UpdateArgs {
    /// Issue id or unique prefix (at least four characters)
    id: String,
    /// Agent name
    #[arg(long)]
    author: AuthorName,
    /// The best-known cause (at most 600 characters)
    #[arg(long)]
    cause: Option<String>,
    /// The current best fix or mitigation (at most 600 characters)
    #[arg(long)]
    fix: Option<String>,
    /// The new status
    #[arg(long, value_name = "STATUS", value_parser = clap::builder::PossibleValuesParser::new(IssueStatus::SETTABLE))]
    status: Option<String>,
    /// The request this update answers, if Daniel made one: checked as for add
    #[command(flatten)]
    request: RequestArgs,
}

#[derive(Debug, Args)]
#[command(group(clap::ArgGroup::new("destination").required(true).args(["to", "new"])))]
pub(crate) struct MoveArgs {
    /// The occurrence: <issue-id>#<n>
    from: String,
    /// Agent name
    #[arg(long)]
    author: AuthorName,
    /// The issue it belongs to (id or unique prefix)
    #[arg(long, value_name = "ISSUE")]
    to: Option<String>,
    /// File it under a new issue instead; requires --title and --signature
    #[arg(long, requires_all = ["title", "signature"])]
    new: bool,
    /// The new issue's title
    #[arg(long, requires = "new")]
    title: Option<String>,
    /// The new issue's signature, repeatable
    #[arg(long, value_name = "TEXT", requires = "new")]
    signature: Vec<String>,
    /// The new issue's topics, repeatable
    #[arg(long, requires = "new")]
    topic: Vec<Topic>,
    /// The request this move answers, if Daniel made one: checked as for add
    #[command(flatten)]
    request: RequestArgs,
}

#[derive(Debug, Args)]
pub(crate) struct SearchArgs {
    /// The words to look for: symptoms or error text
    terms: String,
    /// Only issues that occurred in this project [default: any]
    #[arg(long, value_name = "PROJECT")]
    project: Option<Project>,
    /// Only issues carrying this topic; repeat it to require every one [default: any]
    #[arg(long, value_name = "TOPIC")]
    topic: Vec<Topic>,
    /// Show at most this many hits, 1 or more [default: 20]
    #[arg(long, value_name = "N")]
    limit: Option<NonZeroUsize>,
}

#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// Issue id or unique prefix (at least four characters)
    id: String,
}

#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Only issues with this status; forgotten issues are listed only when asked for [default: any but forgotten]
    #[arg(long, value_name = "STATUS", value_parser = clap::builder::PossibleValuesParser::new(IssueStatus::NAMES))]
    status: Option<String>,
    /// Only issues that occurred in this project [default: any]
    #[arg(long, value_name = "PROJECT")]
    project: Option<Project>,
    /// Only issues that occurred on this host [default: any]
    #[arg(long, value_name = "HOST", value_parser = host_parser)]
    host: Option<Label>,
    /// Show at most this many issues, 1 or more [default: all]
    #[arg(long, value_name = "N")]
    limit: Option<NonZeroUsize>,
}

/// A host label, as an occurrence records it.
fn host_parser(text: &str) -> Result<Label, String> {
    Label::try_from(text.to_owned()).map_err(|e| e.to_string())
}

impl IssueCommand {
    /// The issue and agent a write names. See [`super::Command::subject`].
    pub(super) fn subject(&self, ctx: &Context) -> AuditSubject {
        let author = |a: &AuthorName| Some(a.as_str().to_owned());
        match self {
            Self::Search(_) | Self::Show(_) | Self::List(_) => AuditSubject::default(),
            Self::Add(args) => AuditSubject {
                target: None,
                author: author(&args.author),
            },
            Self::Occur(args) => AuditSubject {
                target: resolved(ctx, &args.id),
                author: author(&args.author),
            },
            Self::Update(args) => AuditSubject {
                target: resolved(ctx, &args.id),
                author: author(&args.author),
            },
            Self::Move(args) => AuditSubject {
                target: resolved(ctx, &args.from),
                author: author(&args.author),
            },
        }
    }

    /// Every write is audited; search, show and list read only. See
    /// [`super::Command::audit`].
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        let meta = |command| Some(AuditMeta { command });
        match self {
            Self::Search(_) | Self::Show(_) | Self::List(_) => None,
            Self::Add(_) => meta("issue add"),
            Self::Occur(_) => meta("issue occur"),
            Self::Update(_) => meta("issue update"),
            Self::Move(_) => meta("issue move"),
        }
    }
}

/// The id of the issue `input` resolves to, or `None` when it does not.
fn resolved(ctx: &Context, input: &str) -> Option<String> {
    ctx.store()
        .resolve_issue(input)
        .ok()
        .map(|(issue, _)| issue.id.to_string())
}

/// Run one `issue` verb.
pub(super) fn run(command: IssueCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        IssueCommand::Add(args) => add(args, ctx),
        IssueCommand::Occur(args) => occur(args, ctx),
        IssueCommand::Update(args) => update(args, ctx),
        IssueCommand::Move(args) => move_occurrence(args, ctx),
        IssueCommand::Search(args) => search(args, ctx),
        IssueCommand::Show(args) => {
            let (issue, _) = ctx.store().resolve_issue(&args.id)?;
            Ok(Output::Show {
                record: issue_record(&issue),
            })
        }
        IssueCommand::List(args) => list(args, ctx),
    }
}

fn issue_record(issue: &scribe_core::issue::Issue) -> Record {
    Record::Issue(Box::new(issue.into()))
}

fn refuse(kind: WriteRefusal, message: impl Into<String>) -> CliError {
    scribe_core::Error::AgentWrite {
        kind,
        message: message.into(),
    }
    .into()
}

/// The request a write carries, once checked: its session, the uuid of
/// Daniel's typed turn that holds it, and the words.
struct Request {
    session: SessionId,
    message: String,
    words: String,
}

/// Check `args`' request: word for word in one of Daniel's typed turns of a
/// session `--related` names, in that session's transcript on this host
/// (§4.6's quote check). The session and the turn are noted for the audit
/// line as each is known.
///
/// # Errors
///
/// `quote_required` with no request or no session named; `quote_not_found`
/// when no named session's transcript holds it.
fn check_request(ctx: &Context, verb: &str, args: &RequestArgs) -> Result<Request, CliError> {
    let sessions: Vec<SessionId> = args
        .related
        .iter()
        .filter(|r| r.kind() == RelatedKind::Session)
        .filter_map(|r| r.target().parse().ok())
        .collect();
    if let Some(first) = sessions.first() {
        ctx.note_issue(verb, |a| a.session = Some(first.to_string()));
    }
    let Some(words) = args.request.as_deref().filter(|w| !w.trim().is_empty()) else {
        return Err(refuse(
            WriteRefusal::QuoteRequired,
            "an issue write carries Daniel's request word for word; pass --request '<his words>' with --related session:<id>",
        ));
    };
    if sessions.is_empty() {
        return Err(refuse(
            WriteRefusal::QuoteRequired,
            "the request is checked against the session it was typed in; pass --related session:<id>",
        ));
    }
    for id in &sessions {
        if let Some(message) = find_request(ctx, id, words)? {
            ctx.note_issue(verb, |a| {
                a.session = Some(id.to_string());
                a.request_message = Some(message.clone());
            });
            return Ok(Request {
                session: id.clone(),
                message,
                words: words.to_owned(),
            });
        }
    }
    Err(refuse(
        WriteRefusal::QuoteNotFound,
        "the request is not word for word in Daniel's typed turns of the session named, or its transcript is not readable on this host; quote his own words exactly",
    ))
}

/// The uuid of Daniel's typed turn in session `id` whose own words hold
/// `words`, or `None` when its transcript is missing, fails the provenance
/// check or holds no such turn.
fn find_request(ctx: &Context, id: &SessionId, words: &str) -> Result<Option<String>, CliError> {
    use super::hook::payload;
    let Some(path) = payload::locate(ctx, id)? else {
        return Ok(None);
    };
    if !matches!(
        payload::check(ctx, &path, id),
        Ok(payload::Transcript::Present)
    ) {
        return Ok(None);
    }
    let bytes = match payload::read_for(ctx, &path) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "cannot read the transcript");
            return Ok(None);
        }
    };
    let messages = scribe_core::transcript::parse(&bytes);
    Ok(messages.iter().find_map(|m| {
        scribe_core::transcript::daniel_turn(m)
            .filter(|turn| scribe_core::verbatim::contains(&turn.own, words))
            .map(|_| m.uuid.clone())
    }))
}

/// The occurrence a write records, from its flags and its checked request.
/// Every `--related` reference but the request's session is a link.
fn new_occurrence(
    ctx: &Context,
    request: Request,
    related: Vec<Related>,
    args: OccurrenceArgs,
) -> NewOccurrence {
    NewOccurrence {
        date: ctx.now().date(),
        host: ctx.host(),
        project: args.project,
        session: request.session,
        request_message: request.message,
        request: request.words,
        summary: args.summary,
        error_lines: args.error_line,
        cause: args.cause,
        fix: args.fix,
        links: related,
    }
}

fn add(args: AddArgs, ctx: &Context) -> Result<Output, CliError> {
    ctx.note_issue("add", |_| {});
    scribe_core::check_agent_author(&args.author)?;
    let topics = Topics::try_from(args.topic).map_err(|e| CliError::Usage(e.to_string()))?;
    let new = NewIssue {
        title: args.title,
        signature: args.signature,
        topics,
        cause: args.occurrence.cause.clone(),
        fix: args.occurrence.fix.clone(),
    };
    // The bounds first: a field out of bounds is reported before the
    // transcript is read.
    new.check()?;
    let request = check_request(ctx, "add", &args.request)?;
    let occurrence = new_occurrence(ctx, request, args.request.related, args.occurrence);
    occurrence
        .clone()
        .check(Author::agent(args.author.clone()))?;
    let redactor = ctx.redactor(&ctx.store().config()?)?;
    let issue = ctx.store().add_issue(
        &new,
        occurrence,
        Author::agent(args.author),
        ctx.now(),
        &redactor,
    )?;
    let n = issue
        .occurrences
        .first()
        .map_or(std::num::NonZeroU32::MIN, |o| o.n);
    ctx.note_issue("add", |a| {
        a.id = Some(issue.id.to_string());
        a.occurrence = Some(n.get());
    });
    Ok(written(&issue, true, vec![cancel_line(&issue, n)]))
}

fn occur(args: OccurArgs, ctx: &Context) -> Result<Output, CliError> {
    ctx.note_issue("occur", |a| a.id = resolved(ctx, &args.id));
    scribe_core::check_agent_author(&args.author)?;
    let request = check_request(ctx, "occur", &args.request)?;
    let occurrence = new_occurrence(ctx, request, args.request.related, args.occurrence);
    occurrence
        .clone()
        .check(Author::agent(args.author.clone()))?;
    let redactor = ctx.redactor(&ctx.store().config()?)?;
    let (issue, n) = ctx.store().occur_issue(
        &args.id,
        occurrence,
        Author::agent(args.author),
        ctx.now(),
        &redactor,
    )?;
    ctx.note_issue("occur", |a| {
        a.id = Some(issue.id.to_string());
        a.occurrence = Some(n.get());
    });
    let mut notes = vec![cancel_line(&issue, n)];
    if issue.occurrence(n).is_some_and(|o| o.recurred) {
        notes.push(format!("{} recurred after its fix: reopened", issue.id));
    }
    Ok(written(&issue, true, notes))
}

/// An optional request: checked when given, `None` when neither it nor a
/// session is named.
fn optional_request(ctx: &Context, verb: &str, args: &RequestArgs) -> Result<(), CliError> {
    if args.request.is_some() {
        check_request(ctx, verb, args)?;
    } else if let Some(session) = args
        .related
        .iter()
        .find(|r| r.kind() == RelatedKind::Session)
    {
        ctx.note_issue(verb, |a| a.session = Some(session.target().to_owned()));
    }
    Ok(())
}

fn update(args: UpdateArgs, ctx: &Context) -> Result<Output, CliError> {
    ctx.note_issue("update", |a| a.id = resolved(ctx, &args.id));
    scribe_core::check_agent_author(&args.author)?;
    let status = args
        .status
        .as_deref()
        .map(str::parse::<IssueStatus>)
        .transpose()
        .map_err(CliError::Usage)?;
    let patch = IssuePatch {
        cause: args.cause,
        fix: args.fix,
        status,
    };
    patch.check()?;
    optional_request(ctx, "update", &args.request)?;
    let redactor = ctx.redactor(&ctx.store().config()?)?;
    let (issue, changed) = ctx.store().update_issue(
        &args.id,
        &patch,
        Author::agent(args.author),
        ctx.now(),
        &redactor,
    )?;
    let notes = if changed {
        Vec::new()
    } else {
        vec![String::from("nothing changed")]
    };
    Ok(written(&issue, changed, notes))
}

fn move_occurrence(args: MoveArgs, ctx: &Context) -> Result<Output, CliError> {
    ctx.note_issue("move", |a| {
        a.id = resolved(ctx, &args.from);
        a.occurrence = args
            .from
            .split_once('#')
            .and_then(|(_, n)| n.parse::<u32>().ok());
    });
    scribe_core::check_agent_author(&args.author)?;
    let to = match (args.to, args.new) {
        (Some(to), _) => MoveTo::Issue(to),
        (None, _) => MoveTo::New {
            title: args.title.unwrap_or_default(),
            signature: args.signature,
            topics: Topics::try_from(args.topic).map_err(|e| CliError::Usage(e.to_string()))?,
        },
    };
    optional_request(ctx, "move", &args.request)?;
    let redactor = ctx.redactor(&ctx.store().config()?)?;
    let moved = ctx.store().move_occurrence(
        &args.from,
        &to,
        Author::agent(args.author),
        ctx.now(),
        &redactor,
    )?;
    let to_ref = format!("{}#{}", moved.to.id, moved.to_n);
    ctx.note_issue("move", |a| {
        a.id = Some(moved.from.id.to_string());
        a.occurrence = Some(moved.from_n.get());
        a.moved_to = Some(to_ref.clone());
    });
    Ok(written(
        &moved.to,
        true,
        vec![format!(
            "moved {}#{} to {to_ref}",
            moved.from.id, moved.from_n
        )],
    ))
}

/// A write's output: the issue as it now stands, with its stderr lines.
fn written(issue: &scribe_core::issue::Issue, changed: bool, notes: Vec<String>) -> Output {
    let mut output = Output::written(issue_record(issue), changed);
    if let Output::Written { notices, .. } = &mut output {
        notices.extend(notes);
    }
    output
}

/// The line an added occurrence prints: how Daniel cancels it.
fn cancel_line(issue: &scribe_core::issue::Issue, n: std::num::NonZeroU32) -> String {
    format!(
        "recorded {}#{n}; Daniel cancels with #scribe forget {}#{n}, or #scribe forget {} for the whole issue",
        issue.id, issue.id, issue.id
    )
}

fn search(args: SearchArgs, ctx: &Context) -> Result<Output, CliError> {
    let limit = args
        .limit
        .or(NonZeroUsize::new(DEFAULT_LIMIT))
        .unwrap_or(NonZeroUsize::MIN);
    let query = SearchQuery {
        terms: args.terms,
        kind: Some(RecordKind::Issue),
        project: args.project,
        topics: args.topic,
        dates: DateRange::default(),
        include_hidden: false,
        limit,
    };
    let store = ctx.store();
    let config = store.config()?;
    let (issues, skipped) = store.issues()?;
    let corpus = Corpus {
        entries: &[],
        sessions: &[],
        issues: &issues,
        tags: &config.tags,
    };
    let found = search::search(&query, corpus).map_err(|e| match e {
        SearchError::NoTerms(_) => CliError::Usage(e.to_string()),
    })?;
    let mut criteria = vec![format!("{:?}", query.terms)];
    if let Some(project) = &query.project {
        criteria.push(format!("--project {project}"));
    }
    for topic in &query.topics {
        criteria.push(format!("--topic {topic}"));
    }
    let empty = format!(
        "no issues in {}; an agent records one with scribe issue add",
        store.root().display()
    );
    let notice = ListNotice {
        noun: "issues",
        criteria,
        empty: &empty,
    };
    let records = Records::Hits(found.hits.iter().map(HitPayload::from).collect());
    Ok(list_output(records, found.total, &skipped, &notice))
}

fn list(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let status = args
        .status
        .as_deref()
        .map(str::parse::<IssueStatus>)
        .transpose()
        .map_err(CliError::Usage)?;
    let filter = IssueFilter {
        status,
        project: args.project,
        host: args.host,
        limit: args.limit,
    };
    let found = ctx.store().list_issues(&filter)?;
    let mut criteria = Vec::new();
    if let Some(status) = filter.status {
        criteria.push(format!("--status {status}"));
    }
    if let Some(project) = &filter.project {
        criteria.push(format!("--project {project}"));
    }
    if let Some(host) = &filter.host {
        criteria.push(format!("--host {host}"));
    }
    let empty = format!(
        "no issues in {}; an agent records one with scribe issue add",
        ctx.store().root().display()
    );
    let notice = ListNotice {
        noun: "issues",
        criteria,
        empty: &empty,
    };
    let records = Records::Issues(
        found
            .listing
            .items
            .iter()
            .map(crate::output::IssuePayload::from)
            .collect(),
    );
    Ok(list_output(
        records,
        found.listing.total,
        &found.skipped,
        &notice,
    ))
}
