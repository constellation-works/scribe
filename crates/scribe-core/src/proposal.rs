//! Proposals: what the consistency routine suggests changing, held until
//! Daniel settles each with `#scribe approve` or `#scribe reject`
//! (docs/design/scribe/2_design.md §7). Nothing changes on an entry until a
//! proposal is approved. Pure values (STD-02 R14) and no I/O (STD-02 R5).
//!
//! The serde shapes are persisted in `proposals.json` (STD-02 R16): every key
//! is required and an unknown one is refused.

use crate::entry::{Cite, EntryId};
use crate::record::{Author, nullable};
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
}

impl ProposalKind {
    /// The canonical lowercase token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supersedes => "supersedes",
            Self::Stale => "stale",
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
    /// The entry that would be superseded or retired.
    pub older: EntryId,
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
