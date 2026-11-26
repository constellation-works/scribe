//! Proposals from the consistency routine and opted-in session passes, held until
//! Daniel settles each with `#scribe approve` or `#scribe reject`
//! (docs/design/scribe/2_design.md §7). Nothing changes on an entry until a
//! proposal is approved. Pure values (STD-02 R14) and no I/O (STD-02 R5).
//!
//! The serde shapes are persisted in `proposals.json` (STD-02 R16): every key
//! is required and an unknown one is refused.

use crate::entry::{Cite, EntryId, Instruction, Speaker, TagName};
use crate::record::{Author, Label, Project, nullable};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::num::NonZeroU64;
use std::str::FromStr;
use time::OffsetDateTime;

/// The longest context paragraph a pass claim carries, in characters
/// (2_design.md §15.3).
pub const CONTEXT_CHARS: usize = 600;
/// The fewest sentences a context paragraph has; a shorter one does not
/// stand alone and opens nothing (2_design.md §15.3).
pub const CONTEXT_SENTENCES: usize = 2;
/// The longest subject a pass claim carries, in characters (2_design.md §11.1).
pub const SUBJECT_CHARS: usize = 80;

/// How the `said` of an entry approved on Daniel's instruction begins: the
/// canonical `proposal settle` command, where a typed approval's begins
/// `#scribe approve p-` (2_design.md §12.3).
pub const INSTRUCTED_SAID_PREFIX: &str = "scribe proposal settle p-";

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
    /// The span; it passes `scribe_tools::text::verbatim::contains` against the entry.
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
    /// Why scribe itself closed it, such as `tag_retired`; `null` when
    /// Daniel or an agent settled it, or it is open (format 11).
    #[serde(deserialize_with = "nullable")]
    pub closed_reason: Option<ClosedReason>,
    /// Daniel's checked instruction that settled this candidate proposal
    /// (format 13); `null` when Daniel or scribe settled it, or while open.
    #[serde(deserialize_with = "nullable")]
    pub instruction: Option<Instruction>,
}

/// Why a proposal was closed without Daniel's `#scribe` turn (2_design.md §11.3, §12.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedReason {
    /// Its candidate's tag is no longer in the tag table.
    TagRetired,
    /// An agent rejected it on Daniel's checked instruction (§12.4).
    Instructed,
    /// An approved candidate whose reserved entry was never written, closed
    /// because an active entry now covers its statement (format 15). The
    /// approval's settlement, and its instruction when an agent approved it
    /// on Daniel's word, are kept as they were: nobody rejected it.
    Covered,
    /// Its source was erased by a typed session forget.
    Withdrawn,
}

impl ClosedReason {
    /// The canonical token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TagRetired => "tag_retired",
            Self::Instructed => "instructed",
            Self::Covered => "covered",
            Self::Withdrawn => "withdrawn",
        }
    }
}

/// What admitted a pass claim under its tag (2_design.md §11.2): the
/// trigger phrase as configured, the turn's words it matched (verbatim, at
/// most 120 characters) and the tag's `criteria` in force. Written once; a
/// later config change does not rewrite it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admitted {
    /// The trigger phrase, as configured.
    pub trigger: String,
    /// The words of the turn it matched.
    pub matched: String,
    /// The tag's `criteria` when the claim was admitted; `null` when the tag
    /// had none.
    #[serde(deserialize_with = "nullable")]
    pub criteria: Option<String>,
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
    /// The pass's paragraph of context (format 24, 2_design.md §15.3): two
    /// to four sentences of at most [`CONTEXT_CHARS`] characters (counted
    /// before redaction, which may lengthen it) saying what was being
    /// discussed and what the turn settles, standing alone, in the session
    /// model's third-person words, redacted. Approval makes it the entry's
    /// content and `quote` the entry's `quote`. `null` for a claim opened
    /// before format 24.
    #[serde(deserialize_with = "nullable")]
    pub context: Option<String>,
    /// The pass's one-line statement of a claim opened at formats 14 to 23
    /// (2_design.md §11.1): read, shown in the context's place as an older
    /// claim, and approved as content as before; never written from format
    /// 24 on. `null` for a claim opened before statements, which approves
    /// with the quote as content, and for every claim with a `context`.
    #[serde(deserialize_with = "nullable")]
    pub statement: Option<String>,
    /// What the claim is about, as a short noun phrase of at most
    /// [`SUBJECT_CHARS`] characters before redaction (format 14),
    /// redacted; `null` beside a `null` context and statement.
    #[serde(deserialize_with = "nullable")]
    pub subject: Option<String>,
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
    /// The trigger and criteria that admitted it (format 11); `null` for a
    /// candidate opened before capture rules.
    #[serde(deserialize_with = "nullable")]
    pub admitted: Option<Admitted>,
}

impl EntryCandidate {
    /// What scribe says the claim is: its context or, for an older claim,
    /// its statement; `None` for a claim opened before statements.
    #[must_use]
    pub fn stated(&self) -> Option<&str> {
        self.context.as_deref().or(self.statement.as_deref())
    }

    /// Provenance fields a pass derives from the host transcript.
    pub fn has_valid_provenance(&self) -> bool {
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
