//! The machine-output contract (STD-01 R6, R10, R11).
//!
//! Every rendering — table, tab-separated, JSON — is derived from these
//! structs, so the human and machine views cannot disagree about what exists.
//! The JSON field names and types are a public contract: renaming, removing
//! or retyping one is a breaking change, pinned by the output goldens. These
//! shapes are deliberately separate from the store's persisted documents.
//! An absent value is `null`, never an omitted key (R11).

use scribe_core::ask::{Answer, Found, Task};
use scribe_core::{
    Child, Cite, ContentFrom, ControlState, Entry, Proposal, Quote, SessionControl, SessionId,
    SessionRecord, Skipped, TagSummary, Term,
};
use serde::Serialize;
use std::collections::BTreeMap;
use time::OffsetDateTime;

/// RFC 3339 with an explicit offset (R11).
type Timestamp = OffsetDateTime;

/// Where an entry's content came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ContentFromPayload {
    /// `instruction` or `message`.
    pub(crate) kind: &'static str,
    /// `user` or `assistant` for a message; `null` for an instruction.
    pub(crate) role: Option<&'static str>,
    /// The message's uuid; `null` for an instruction.
    pub(crate) message: Option<String>,
    /// When the message was sent; `null` for an instruction.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) at: Option<Timestamp>,
}

impl From<&ContentFrom> for ContentFromPayload {
    fn from(from: &ContentFrom) -> Self {
        match from {
            ContentFrom::Instruction => Self {
                kind: "instruction",
                role: None,
                message: None,
                at: None,
            },
            ContentFrom::Message { role, message, at } => Self {
                kind: "message",
                role: Some(role.as_str()),
                message: Some(message.clone()),
                at: Some(*at),
            },
        }
    }
}

/// The turn a record was typed in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CitePayload {
    pub(crate) session: String,
    pub(crate) message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    pub(crate) prompt_sha256: String,
    pub(crate) line: u32,
    pub(crate) cwd: String,
}

impl From<&Cite> for CitePayload {
    fn from(cite: &Cite) -> Self {
        Self {
            session: cite.session.to_string(),
            message: cite.message.clone(),
            at: cite.at,
            prompt_sha256: cite.prompt_sha256.clone(),
            line: cite.line,
            cwd: cite.cwd.clone(),
        }
    }
}

/// One entry as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct EntryPayload {
    pub(crate) id: String,
    pub(crate) tag: String,
    /// `active`, `superseded`, `retired` or `forgotten`.
    pub(crate) status: &'static str,
    pub(crate) project: String,
    pub(crate) topics: Vec<String>,
    pub(crate) related: Vec<String>,
    pub(crate) provider: String,
    pub(crate) host: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: Timestamp,
    /// The `#scribe` line as typed.
    pub(crate) said: String,
    /// What the entry records, verbatim.
    pub(crate) content: String,
    pub(crate) content_from: ContentFromPayload,
    pub(crate) cite: CitePayload,
    pub(crate) superseded_by: Option<String>,
    /// `YYYY-MM-DD`.
    pub(crate) review_after: Option<String>,
}

fn strings<T: ToString>(items: &[T]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

/// `YYYY-MM-DD`.
fn date(date: time::Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

impl From<&Entry> for EntryPayload {
    fn from(entry: &Entry) -> Self {
        Self {
            id: entry.id.to_string(),
            tag: entry.tag.to_string(),
            status: entry.status.as_str(),
            project: entry.project.to_string(),
            topics: strings(entry.topics.as_slice()),
            related: strings(&entry.related),
            provider: entry.provider.to_string(),
            host: entry.host.to_string(),
            created: entry.created,
            updated: entry.updated,
            said: entry.said.clone(),
            content: entry.content.clone(),
            content_from: ContentFromPayload::from(&entry.content_from),
            cite: CitePayload::from(&entry.cite),
            superseded_by: entry.superseded_by.as_ref().map(ToString::to_string),
            review_after: entry.review_after.map(date),
        }
    }
}

/// A phrase Daniel coined or defined, with his defining words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TermPayload {
    pub(crate) term: String,
    /// Verbatim from Daniel's turn.
    pub(crate) said: String,
    /// The uuid of that turn's message.
    pub(crate) message: String,
}

impl From<&Term> for TermPayload {
    fn from(t: &Term) -> Self {
        Self {
            term: t.term.clone(),
            said: t.said.clone(),
            message: t.message.clone(),
        }
    }
}

/// A link a session record stores: how it relates to another session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct LinkPayload {
    /// `spawned_from` or `continues`.
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    /// The session linked to.
    pub(crate) session: String,
    /// That session's record title; `null` when it has no record.
    pub(crate) title: Option<String>,
}

/// A session that links to this one, derived at read time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ChildPayload {
    /// How the child relates to this session: `spawned_from` or `continues`.
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    /// The child session.
    pub(crate) session: String,
    /// Its record title.
    pub(crate) title: String,
}

impl From<&Child> for ChildPayload {
    fn from(child: &Child) -> Self {
        Self {
            kind: child.kind.as_str(),
            session: child.session.to_string(),
            title: child.title.clone(),
        }
    }
}

/// What a session payload shows besides its own record: the sessions that
/// link to it and the titles of the records it links to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Kin<'a> {
    pub(crate) children: &'a [Child],
    pub(crate) titles: &'a BTreeMap<SessionId, String>,
}

impl<'a> Kin<'a> {
    /// `id`'s kin from a listing's derived children and titles.
    pub(crate) fn of(
        id: &SessionId,
        children: &'a BTreeMap<SessionId, Vec<Child>>,
        titles: &'a BTreeMap<SessionId, String>,
    ) -> Self {
        Self {
            children: children.get(id).map_or(&[][..], Vec::as_slice),
            titles,
        }
    }
}

/// One session record, with its switch, as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SessionPayload {
    pub(crate) id: String,
    /// `open` or `final`.
    pub(crate) status: &'static str,
    pub(crate) project: String,
    pub(crate) topics: Vec<String>,
    pub(crate) related: Vec<String>,
    /// The links this session stores (`spawned_from`, `continues`).
    pub(crate) links: Vec<LinkPayload>,
    /// The sessions that link to this one, derived at read time.
    pub(crate) children: Vec<ChildPayload>,
    /// Daniel's own words, unlike the four model-written sections.
    pub(crate) terms: Vec<TermPayload>,
    pub(crate) provider: String,
    pub(crate) host: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: Timestamp,
    pub(crate) title: String,
    pub(crate) cwd: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) started: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) on_since: Timestamp,
    pub(crate) scribed_through: Option<String>,
    pub(crate) passes: u32,
    pub(crate) next_open: u32,
    /// Model-written, never Daniel's words.
    pub(crate) summary: String,
    pub(crate) goals: String,
    pub(crate) outcome: String,
    pub(crate) next: String,
    /// Whether cataloging is on now (`control.json`); `false` when the
    /// session has no switch.
    pub(crate) on: bool,
    /// Whether a pass is due and not yet run.
    pub(crate) due: bool,
}

impl SessionPayload {
    pub(crate) fn new(
        record: &SessionRecord,
        control: Option<&SessionControl>,
        kin: Kin<'_>,
    ) -> Self {
        Self {
            id: record.id.to_string(),
            status: record.status.as_str(),
            project: record.project.to_string(),
            topics: strings(record.topics.as_slice()),
            related: strings(&record.related),
            links: record
                .links
                .iter()
                .map(|l| LinkPayload {
                    kind: l.kind.as_str(),
                    session: l.session.to_string(),
                    title: kin.titles.get(&l.session).cloned(),
                })
                .collect(),
            children: kin.children.iter().map(ChildPayload::from).collect(),
            terms: record.terms.iter().map(TermPayload::from).collect(),
            provider: record.provider.to_string(),
            host: record.host.to_string(),
            created: record.created,
            updated: record.updated,
            title: record.title.clone(),
            cwd: record.cwd.clone(),
            started: record.started,
            on_since: record.on_since,
            scribed_through: record.scribed_through.clone(),
            passes: record.passes,
            next_open: record.next_open,
            summary: record.summary.clone(),
            goals: record.goals.clone(),
            outcome: record.outcome.clone(),
            next: record.next.clone(),
            on: control.is_some_and(|c| c.on),
            due: control.is_some_and(|c| c.due),
        }
    }
}

/// A verbatim span a proposal rests on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct QuotePayload {
    pub(crate) entry: String,
    pub(crate) text: String,
}

impl From<&Quote> for QuotePayload {
    fn from(quote: &Quote) -> Self {
        Self {
            entry: quote.entry.to_string(),
            text: quote.text.clone(),
        }
    }
}

/// One proposal as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ProposalPayload {
    pub(crate) id: String,
    /// `supersedes` or `stale`.
    pub(crate) kind: &'static str,
    pub(crate) older: String,
    pub(crate) newer: Option<String>,
    /// Model text.
    pub(crate) reason: String,
    pub(crate) quotes: Vec<QuotePayload>,
    /// `open`, `approved` or `rejected`.
    pub(crate) state: &'static str,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) opened_at: Timestamp,
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) settled_at: Option<Timestamp>,
    pub(crate) settled_by: Option<CitePayload>,
}

impl From<&Proposal> for ProposalPayload {
    fn from(p: &Proposal) -> Self {
        Self {
            id: p.id.to_string(),
            kind: p.kind.as_str(),
            older: p.older.to_string(),
            newer: p.newer.as_ref().map(ToString::to_string),
            reason: p.reason.clone(),
            quotes: p.quotes.iter().map(QuotePayload::from).collect(),
            state: p.state.as_str(),
            opened_at: p.opened_at,
            settled_at: p.settled_at,
            settled_by: p.settled_by.as_ref().map(CitePayload::from),
        }
    }
}

/// One tag as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TagPayload {
    pub(crate) name: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) description: Option<String>,
    /// `null` when entries never go stale.
    pub(crate) stale_after_days: Option<u32>,
    /// Whether `config.toml` (or the defaults) declares it; `false` for a
    /// directory of entries no configured tag names.
    pub(crate) configured: bool,
    /// Entries in it, any status.
    pub(crate) entries: usize,
}

impl From<&TagSummary> for TagPayload {
    fn from(tag: &TagSummary) -> Self {
        Self {
            name: tag.name.to_string(),
            aliases: tag
                .def
                .as_ref()
                .map(|d| strings(&d.aliases))
                .unwrap_or_default(),
            description: tag.def.as_ref().and_then(|d| d.description.clone()),
            stale_after_days: tag
                .def
                .as_ref()
                .and_then(|d| d.stale_after_days)
                .map(|d| d.get()),
            configured: tag.def.is_some(),
            entries: tag.entries,
        }
    }
}

/// A record file a list left out because it did not parse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WarningPayload {
    /// The file.
    pub(crate) path: String,
    /// The stable error code, `store_corrupt`.
    pub(crate) code: &'static str,
    /// What was wrong, naming the file.
    pub(crate) error: String,
}

impl From<&Skipped> for WarningPayload {
    fn from(skipped: &Skipped) -> Self {
        let mut error = skipped.error.to_string();
        let mut source = std::error::Error::source(&skipped.error);
        while let Some(cause) = source {
            error.push_str(": ");
            error.push_str(&cause.to_string());
            source = cause.source();
        }
        Self {
            path: skipped.path.display().to_string(),
            code: skipped.error.code(),
            error,
        }
    }
}

/// The records a list returned, by kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Records {
    Entries(Vec<EntryPayload>),
    Sessions(Vec<SessionPayload>),
    /// `session list --tree`: sessions in tree order, each with its depth.
    SessionTree(Vec<TreeRow>),
    Proposals(Vec<ProposalPayload>),
    Tags(Vec<TagPayload>),
}

impl Records {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Entries(v) => v.len(),
            Self::Sessions(v) => v.len(),
            Self::SessionTree(v) => v.len(),
            Self::Proposals(v) => v.len(),
            Self::Tags(v) => v.len(),
        }
    }
}

/// One row of `session list --tree`. Its JSON is the session's alone: the
/// nesting is in each session's `links` and `children`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct TreeRow {
    /// How deep it nests: 0 for a root.
    #[serde(skip)]
    pub(crate) depth: usize,
    /// The session it nests under, if it has one in the list.
    #[serde(skip)]
    pub(crate) parent: Option<String>,
    pub(crate) session: SessionPayload,
}

/// One record a `show` returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Record {
    Entry(Box<EntryPayload>),
    Session(Box<SessionPayload>),
}

/// The reply a Claude Code hook prints: the same text for Daniel
/// (`systemMessage`) and for the main agent (`additionalContext`), plus what
/// the audit log records for the run. Its JSON shape is Claude Code's hook
/// protocol, so it is the same in every output mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HookReply {
    /// The hook event, such as `UserPromptSubmit`.
    pub(crate) event: &'static str,
    /// Shown to Daniel; `None` prints no reply at all.
    pub(crate) message: Option<String>,
    /// Shown to the main agent beside the prompt.
    pub(crate) context: Option<String>,
    /// Block the prompt with this reason, shown to Daniel: a `/scribe`
    /// command is answered here and never reaches the model.
    pub(crate) block: Option<String>,
    /// One per record written or refused, for the audit log; empty when the
    /// hook changed nothing.
    pub(crate) outcomes: Vec<HookOutcome>,
}

/// One write or refusal a hook run made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HookOutcome {
    /// The record it acted on: an entry id, or `session:<id>` for a refusal.
    pub(crate) target: Option<String>,
    /// The stable code of a refusal; `None` for a write.
    pub(crate) error_code: Option<&'static str>,
}

/// The JSON a hook prints (Claude Code's hook output).
#[derive(Serialize)]
pub(crate) struct HookJson<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) decision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<&'a str>,
    #[serde(rename = "systemMessage", skip_serializing_if = "Option::is_none")]
    pub(crate) system_message: Option<&'a str>,
    #[serde(rename = "hookSpecificOutput", skip_serializing_if = "Option::is_none")]
    pub(crate) hook_specific_output: Option<HookSpecificJson<'a>>,
}

/// `hookSpecificOutput`.
#[derive(Serialize)]
pub(crate) struct HookSpecificJson<'a> {
    #[serde(rename = "hookEventName")]
    pub(crate) hook_event_name: &'a str,
    #[serde(rename = "additionalContext")]
    pub(crate) additional_context: &'a str,
}

/// What a session pass did for one session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PassPayload {
    /// The session id.
    pub(crate) session: String,
    /// `written` (a record was written), `unchanged` (nothing new to
    /// record), `waiting` (no transcript or messages yet; still due) or
    /// `failed` (the previous record is kept).
    pub(crate) result: &'static str,
    /// The stable code of a failure; `null` otherwise.
    pub(crate) error_code: Option<&'static str>,
    /// The record's pass count after this pass; `null` when there is no
    /// record.
    pub(crate) passes: Option<u32>,
    /// Terms the model offered that were not verbatim from Daniel's turns.
    pub(crate) dropped_terms: usize,
    /// Successfully persisted model windows for this session in this run.
    pub(crate) windows: usize,
    /// Always `false`: a failed pass stays due and is never given up. Kept
    /// because the `--json` shape is a contract.
    pub(crate) gave_up: bool,
}

/// What `session pass` did; `--json` and the `scribe.session-pass` tool
/// print it as `{"passes", "remaining"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PassReport {
    /// One per session passed, in id order.
    pub(crate) passes: Vec<PassPayload>,
    /// Due sessions left for the next run.
    pub(crate) remaining: usize,
    /// Lines for stderr, printed in every mode.
    #[serde(skip)]
    pub(crate) notices: Vec<String>,
}

impl PassReport {
    /// The run's error when any pass failed, naming each failed session and
    /// its code; `None` when none did.
    pub(crate) fn failure(&self) -> Option<crate::error::CliError> {
        let failed: Vec<(String, &'static str)> = self
            .passes
            .iter()
            .filter_map(|p| Some((p.session.clone(), p.error_code?)))
            .collect();
        (!failed.is_empty()).then(|| crate::error::CliError::PassFailed(failed))
    }
}

/// An approved proposal a scan applied again, because its older entry was
/// still `active`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ReappliedPayload {
    /// The proposal id.
    pub(crate) proposal: String,
    /// The older entry's id.
    pub(crate) entry: String,
    /// Its status now: `superseded` or `retired`.
    pub(crate) status: &'static str,
}

/// A batch of pairs the consistency model did not label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct BatchFailure {
    /// The batch's number in this scan, from 1.
    pub(crate) batch: usize,
    /// The model failure's stable code.
    pub(crate) error_code: &'static str,
}

/// What `consistency scan` did; `--json` and the `scribe.consistency-scan`
/// tool print it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ScanReport {
    /// Approvals applied again, by proposal id.
    pub(crate) reapplied: Vec<ReappliedPayload>,
    /// Proposals this scan opened, by id.
    pub(crate) opened: Vec<ProposalPayload>,
    /// Candidate pairs the model labelled.
    pub(crate) judged_pairs: usize,
    /// Candidate pairs left for a later scan: past the per-scan cap, or not
    /// reached in this call's time.
    pub(crate) pairs_left: usize,
    /// Pairs labelled `unrelated`.
    pub(crate) unrelated_pairs: usize,
    /// Labels dropped: a quote not verbatim in its entry, an unknown pair or
    /// label.
    pub(crate) dropped_labels: usize,
    /// Stale entries found (each opens a proposal unless one exists).
    pub(crate) stale_entries: usize,
    /// Batches the model failed; their pairs are offered again next scan.
    pub(crate) failed_batches: Vec<BatchFailure>,
    /// Lines for stderr, printed in every mode.
    #[serde(skip)]
    pub(crate) notices: Vec<String>,
}

/// A claim an answer rests on: verbatim-checked against its cite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ClaimPayload {
    /// The statement (model text).
    pub(crate) text: String,
    /// Words verbatim from the cited record.
    pub(crate) quote: String,
    /// An entry id, `session:<id>` or `task:<id>`.
    pub(crate) cite: String,
}

/// Two or more records that disagree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ConflictPayload {
    pub(crate) cites: Vec<String>,
    /// Model text.
    pub(crate) note: String,
}

/// An Orbit task a lookup returned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TaskPayload {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) tags: Vec<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) updated: Option<Timestamp>,
    /// Verbatim; empty when the task has none.
    pub(crate) execution_summary: String,
}

impl From<&Task> for TaskPayload {
    fn from(t: &Task) -> Self {
        Self {
            id: t.id.clone(),
            title: t.title.clone(),
            status: t.status.clone(),
            tags: t.tags.clone(),
            updated: t.updated,
            execution_summary: t.execution_summary.clone(),
        }
    }
}

/// One stored record a lookup returned, as stored: exactly one of `entry`,
/// `session` and `task` is set, as `kind` says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct FoundPayload {
    /// `entry`, `session` or `task`.
    pub(crate) kind: &'static str,
    /// How an answer cites it.
    pub(crate) cite: String,
    pub(crate) entry: Option<EntryPayload>,
    /// The same shape `session show --json` prints.
    pub(crate) session: Option<SessionPayload>,
    pub(crate) task: Option<TaskPayload>,
    /// For a term lookup, the session's terms that matched, with Daniel's
    /// defining words; empty otherwise.
    pub(crate) terms: Vec<TermPayload>,
}

impl FoundPayload {
    fn new(found: &Found, sessions: &SessionsContext<'_>) -> Self {
        let cite = found.cite();
        match found {
            Found::Entry(entry) => Self {
                kind: "entry",
                cite,
                entry: Some(EntryPayload::from(entry)),
                session: None,
                task: None,
                terms: Vec::new(),
            },
            Found::Session { record, terms } => Self {
                kind: "session",
                cite,
                entry: None,
                session: Some(SessionPayload::new(
                    record,
                    sessions.control.get(&record.id),
                    Kin::of(&record.id, sessions.children, sessions.titles),
                )),
                task: None,
                terms: terms.iter().map(TermPayload::from).collect(),
            },
            Found::Task(task) => Self {
                kind: "task",
                cite,
                entry: None,
                session: None,
                task: Some(TaskPayload::from(task)),
                terms: Vec::new(),
            },
        }
    }
}

/// What a found session record is shown with: every switch, the derived
/// children and every record's title.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionsContext<'a> {
    pub(crate) control: &'a ControlState,
    pub(crate) children: &'a BTreeMap<SessionId, Vec<Child>>,
    pub(crate) titles: &'a BTreeMap<SessionId, String>,
}

/// What `question ask` and the `scribe.ask` tool return (2_design.md §6.1):
/// the same payload from both.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AskPayload {
    /// `answered`, `no_record`, `conflict` or `record` (a lookup).
    pub(crate) status: &'static str,
    /// Model text for an answer; a fixed sentence otherwise.
    pub(crate) answer: String,
    pub(crate) claims: Vec<ClaimPayload>,
    pub(crate) conflicts: Vec<ConflictPayload>,
    /// Evidence items the model was given; records returned, for a lookup.
    pub(crate) evidence_considered: usize,
    /// Claims dropped because their quote or cite failed the check.
    pub(crate) dropped_quotes: usize,
    /// Superseded or retired entries that matched and were left out.
    pub(crate) hidden_matches: usize,
    /// The lookup that answered, or `null` when retrieval ran.
    pub(crate) lookup: Option<&'static str>,
    /// The stored records a lookup returned; empty otherwise.
    pub(crate) records: Vec<FoundPayload>,
    /// Record files left out because they did not parse.
    pub(crate) warnings: Vec<WarningPayload>,
}

impl AskPayload {
    pub(crate) fn new(
        answer: &Answer,
        sessions: &SessionsContext<'_>,
        skipped: &[Skipped],
    ) -> Self {
        Self {
            status: answer.status.as_str(),
            answer: answer.answer.clone(),
            claims: answer
                .claims
                .iter()
                .map(|c| ClaimPayload {
                    text: c.text.clone(),
                    quote: c.quote.clone(),
                    cite: c.cite.clone(),
                })
                .collect(),
            conflicts: answer
                .conflicts
                .iter()
                .map(|c| ConflictPayload {
                    cites: c.cites.clone(),
                    note: c.note.clone(),
                })
                .collect(),
            evidence_considered: answer.evidence_considered,
            dropped_quotes: answer.dropped_quotes,
            hidden_matches: answer.hidden_matches,
            lookup: answer.lookup.map(|l| l.as_str()),
            records: answer
                .records
                .iter()
                .map(|f| FoundPayload::new(f, sessions))
                .collect(),
            warnings: skipped.iter().map(WarningPayload::from).collect(),
        }
    }
}

/// A plugin tool call's reply: Orbit's exec-backend envelope, one line on
/// stdout whatever happened (`{"ok":true,"output":...}` or
/// `{"ok":false,"error":{...}}`), in every output mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PluginReply {
    /// `scribe.ask` answered.
    Ask(Box<AskPayload>),
    /// `scribe.session-pass` ran.
    Pass(PassReport),
    /// `scribe.consistency-scan` ran.
    Scan(Box<ScanReport>),
    /// The call failed; the envelope carries the code.
    Error(PluginError),
}

/// A failed plugin call, as the envelope's `error` object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PluginError {
    /// A stable `snake_case` code.
    pub(crate) code: &'static str,
    pub(crate) message: String,
    /// Whether the same call may succeed later.
    pub(crate) retryable: bool,
}

/// What a command produced, before rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    /// A list result.
    List {
        /// The records returned, in order.
        records: Records,
        /// How many matched before the limit; more than the records when the
        /// list was cut.
        total: usize,
        /// Files left out because they did not parse; each is also a stderr
        /// line.
        warnings: Vec<WarningPayload>,
        /// Lines for stderr, printed in every mode (STD-01 R12): what was
        /// searched when nothing matched (R16), or that the list was cut
        /// (R34).
        notices: Vec<String>,
    },
    /// A single record (`show`).
    Show {
        /// The record.
        record: Record,
    },
    /// A hook's reply.
    Hook(HookReply),
    /// A session pass's report.
    Pass(PassReport),
    /// A consistency scan's report.
    Scan(Box<ScanReport>),
    /// An answer to a question.
    Ask(Box<AskPayload>),
    /// A plugin tool call's envelope.
    Plugin(PluginReply),
}
