//! Proposals from the consistency routine and opted-in session passes, held until
//! Daniel settles each with `#scribe approve` or `#scribe reject`
//! (docs/design/scribe/2_design.md §7). Nothing changes on an entry until a
//! proposal is approved. Pure values (STD-02 R14) and no I/O (STD-02 R5).
//!
//! The serde shapes are persisted in `proposals.json` (STD-02 R16): every key
//! is required and an unknown one is refused.

use crate::entry::{Cite, ContentFrom, Entry, EntryId, NewEntry, Speaker, TagName};
use crate::record::{Author, AuthorKind, DateRange, Label, Project, Topics, nullable};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::num::NonZeroU64;
use std::str::FromStr;
use time::OffsetDateTime;

/// Why a value was rejected as a proposal field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProposalError {
    /// The id was not `p-<n>` with a positive `n`.
    #[error("invalid proposal id '{0}': expected p-<n>, such as p-4")]
    InvalidId(String),
}

/// A proposal's id, `p-<n>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProposalId(NonZeroU64);

impl ProposalId {
    /// The number.
    #[must_use]
    pub fn number(self) -> u64 {
        self.0.get()
    }
}

impl TryFrom<String> for ProposalId {
    type Error = ProposalError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value
            .strip_prefix("p-")
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()) && !n.starts_with('0'))
            .and_then(|n| n.parse::<NonZeroU64>().ok())
            .map(Self)
            .ok_or(ProposalError::InvalidId(value))
    }
}

impl From<ProposalId> for String {
    fn from(id: ProposalId) -> Self {
        id.to_string()
    }
}

impl FromStr for ProposalId {
    type Err = ProposalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl fmt::Display for ProposalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "p-{}", self.0)
    }
}

/// What a proposal would change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalKind {
    /// The newer entry supersedes the older one.
    Supersedes,
    /// The older entry is past its `review_after` and would be retired.
    Stale,
    /// Daniel's unmarked words, awaiting his approval as a new entry.
    Candidate,
}

impl ProposalKind {
    /// The canonical lowercase token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supersedes => "supersedes",
            Self::Stale => "stale",
            Self::Candidate => "candidate",
        }
    }
}

/// Whether Daniel has settled it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalState {
    /// Waiting for Daniel.
    Open,
    /// Applied.
    Approved,
    /// Closed for good; the pair is never proposed again.
    Rejected,
}

impl ProposalState {
    /// The canonical lowercase token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

/// A verbatim span of one entry, the evidence for a proposal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quote {
    /// The entry quoted.
    pub entry: EntryId,
    /// The span; it passes [`crate::verbatim::contains`] against the entry.
    pub text: String,
}

/// A stored proposal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// `p-<n>`.
    pub id: ProposalId,
    /// What it would change.
    pub kind: ProposalKind,
    /// The entry that would be superseded or retired; for a candidate,
    /// the reserved id of the entry approval will create.
    pub older: EntryId,
    /// An unmarked statement from an opted-in session; null for consistency proposals.
    #[serde(deserialize_with = "nullable")]
    pub candidate: Option<EntryCandidate>,
    /// The entry that supersedes it; `null` for a stale proposal.
    #[serde(deserialize_with = "nullable")]
    pub newer: Option<EntryId>,
    /// Why, in the consistency model's words (at most 300 characters).
    pub reason: String,
    /// Verbatim spans from the entries involved.
    pub quotes: Vec<Quote>,
    /// Whether it is settled.
    pub state: ProposalState,
    /// When the routine opened it.
    #[serde(with = "time::serde::rfc3339")]
    pub opened_at: OffsetDateTime,
    /// When Daniel settled it.
    #[serde(with = "time::serde::rfc3339::option")]
    pub settled_at: Option<OffsetDateTime>,
    /// The turn Daniel settled it in.
    #[serde(deserialize_with = "nullable")]
    pub settled_by: Option<Cite>,
    /// Who opened it: `{kind: scribe, name: consistency}` for the routine.
    pub author: Author,
    /// Agent who last revised the reason, or null.
    #[serde(deserialize_with = "nullable")]
    pub updated_by: Option<Author>,
    /// Who settled it, set exactly when `state` is not `open`.
    #[serde(deserialize_with = "nullable")]
    pub settled_author: Option<Author>,
}

/// Checked words offered by a session pass: Daniel's unmarked statement, or
/// an assistant turn's measured result or open item (format 10). Metadata
/// is derived from the transcript and session, never supplied by the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryCandidate {
    /// Suggested canonical tag: a memory tag for Daniel's words, or the
    /// fitting tag (`finding`, `remind`, `idea`, ...) for an
    /// assistant turn.
    pub tag: TagName,
    /// Whose turn `quote` is verbatim from. Approval of a `user` candidate
    /// writes Daniel's entry; approval of an `assistant` candidate writes
    /// an agent-authored entry, never one attributed to Daniel.
    pub role: Speaker,
    /// Exact span of that turn, at most 400 characters.
    pub quote: String,
    /// A finding candidate's evidence pointer, copied to the approved
    /// entry; `null` for other tags (the cited turn is the pointer).
    #[serde(deserialize_with = "nullable")]
    pub evidence: Option<String>,
    /// The original turn, including its known message id.
    pub cite: Cite,
    /// Scope inherited from the opted-in session.
    pub project: Project,
    /// Transcript provider.
    pub provider: Label,
    /// Host that read the transcript.
    pub host: Label,
}

impl EntryCandidate {
    /// Provenance fields a pass derives from the host transcript.
    pub(crate) fn has_valid_provenance(&self) -> bool {
        self.cite
            .message
            .as_ref()
            .is_some_and(|id| !id.is_empty() && id.chars().count() <= 128)
            && self.cite.line > 0
            && self.cite.prompt_sha256.len() == 64
            && self
                .cite
                .prompt_sha256
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    }
}

impl Proposal {
    /// Reconstruct an approved candidate's entry, including after an
    /// interrupted approval. The reserved id makes recovery idempotent.
    #[must_use]
    pub(crate) fn candidate_entry(&self, review_after: Option<time::Date>) -> Option<Entry> {
        let candidate = self.candidate.as_ref()?;
        let at = self.settled_at?;
        let message = candidate.cite.message.clone()?;
        let mut entry = NewEntry {
            tag: candidate.tag.clone(),
            project: candidate.project.clone(),
            topics: Topics::default(),
            related: Vec::new(),
            provider: candidate.provider.clone(),
            host: candidate.host.clone(),
            created: at,
            said: format!("#scribe approve {}", self.id),
            content: candidate.quote.clone(),
            content_from: ContentFrom::Message {
                role: candidate.role,
                message,
                at: candidate.cite.at,
            },
            cite: candidate.cite.clone(),
            review_after,
            window: None,
        }
        .into_entry(self.older.clone());
        if candidate.role == Speaker::Assistant {
            // The assistant's own words, kept on Daniel's typed approval:
            // an agent-authored entry, never attributed to Daniel. A
            // finding keeps its evidence pointer (the cited session when
            // the candidate named none) and the turn's date as as-of.
            entry.author = Author::assistant();
            entry.evidence.clone_from(&candidate.evidence);
            if entry.evidence.is_none() && candidate.tag.as_str() == crate::tag::FINDING_TAG {
                entry.evidence = Some(format!("session:{}", candidate.cite.session));
            }
            if entry.evidence.is_some() {
                entry.as_of = Some(candidate.cite.at.date());
            }
        }
        Some(entry)
    }
}

/// Direct proposal change requested by an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalChange {
    /// Replace the reason of an open agent proposal.
    Update,
    /// Apply the proposal to an agent entry.
    Approve,
    /// Reject a proposal about an agent entry.
    Reject,
}

/// Which proposals `proposal list` returns. Every field set must match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProposalFilter {
    /// Only proposals in this state.
    pub state: Option<ProposalState>,
    /// Only proposals opened on a day in this range.
    pub dates: DateRange,
    /// Only proposals about this entry, as the older or the newer one.
    pub entry: Option<EntryId>,
    /// Only proposals opened through this channel.
    pub author_kind: Option<AuthorKind>,
}

impl ProposalFilter {
    /// Whether `proposal` passes every criterion.
    #[must_use]
    pub fn matches(&self, proposal: &Proposal) -> bool {
        self.state.is_none_or(|s| proposal.state == s)
            && self.dates.contains(proposal.opened_at)
            && self
                .entry
                .as_ref()
                .is_none_or(|e| &proposal.older == e || proposal.newer.as_ref() == Some(e))
            && self.author_kind.is_none_or(|k| proposal.author.kind == k)
    }
}
