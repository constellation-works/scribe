//! The machine-output contract (STD-01 R6, R10, R11).
//!
//! Every rendering — table, tab-separated, JSON — is derived from these
//! structs, so the human and machine views cannot disagree about what exists.
//! The JSON field names and types are a public contract: renaming, removing
//! or retyping one is a breaking change, pinned by the output goldens. These
//! shapes are deliberately separate from the store's persisted documents.
//! An absent value is `null`, never an omitted key (R11).

use scribe_core::{
    Cite, ContentFrom, Entry, Proposal, Quote, SessionControl, SessionRecord, Skipped, TagSummary,
    Term,
};
use serde::Serialize;
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

/// One session record, with its switch, as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SessionPayload {
    pub(crate) id: String,
    /// `open` or `final`.
    pub(crate) status: &'static str,
    pub(crate) project: String,
    pub(crate) topics: Vec<String>,
    pub(crate) related: Vec<String>,
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
    pub(crate) fn new(record: &SessionRecord, control: Option<&SessionControl>) -> Self {
        Self {
            id: record.id.to_string(),
            status: record.status.as_str(),
            project: record.project.to_string(),
            topics: strings(record.topics.as_slice()),
            related: strings(&record.related),
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
    Proposals(Vec<ProposalPayload>),
    Tags(Vec<TagPayload>),
}

impl Records {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Entries(v) => v.len(),
            Self::Sessions(v) => v.len(),
            Self::Proposals(v) => v.len(),
            Self::Tags(v) => v.len(),
        }
    }
}

/// One record a `show` returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Record {
    Entry(Box<EntryPayload>),
    Session(Box<SessionPayload>),
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
}
