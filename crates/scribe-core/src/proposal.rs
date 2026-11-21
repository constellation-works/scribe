//! Proposals from the consistency routine and opted-in session passes, held until
//! Daniel settles each with `#scribe approve` or `#scribe reject`
//! (docs/design/scribe/2_design.md §7). Nothing changes on an entry until a
//! proposal is approved. Pure values (STD-02 R14) and no I/O (STD-02 R5).
//!
//! The serde shapes are persisted in `proposals.json` (STD-02 R16): every key
//! is required and an unknown one is refused.

use crate::entry::{Cite, ContentFrom, Entry, EntryId, Instruction, NewEntry, Speaker, TagName};
use crate::record::{Author, AuthorKind, DateRange, Label, Project, Topics, nullable};
use crate::tag::TagTable;
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
}

impl ClosedReason {
    /// The canonical token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TagRetired => "tag_retired",
            Self::Instructed => "instructed",
            Self::Covered => "covered",
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

/// The longest statement a pass claim carries, in characters (2_design.md §11.1).
pub const STATEMENT_CHARS: usize = 300;
/// The longest subject a pass claim carries, in characters (2_design.md §11.1).
pub const SUBJECT_CHARS: usize = 80;

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
    /// The pass's statement of the claim (format 14, 2_design.md §11.1):
    /// one or two sentences of at most [`STATEMENT_CHARS`] characters
    /// (counted before redaction, which may lengthen it) that stand alone, naming the subject and stating the rule or idea, in
    /// the session model's words, redacted. Approval makes it the entry's
    /// content and `quote` the entry's `quote`. `null` for a candidate
    /// opened before statements, which approves with the quote as content.
    #[serde(deserialize_with = "nullable")]
    pub statement: Option<String>,
    /// What the statement is about, as a short noun phrase of at most
    /// [`SUBJECT_CHARS`] characters before redaction (format 14),
    /// redacted; `null` beside a
    /// `null` statement.
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
            quote: None,
            stated_by: None,
        }
        .into_entry(self.older.clone());
        if candidate.role == Speaker::User
            && let Some(statement) = &candidate.statement
        {
            // Scribe's statement of Daniel's turn is the content; his
            // verbatim words stay in `quote`, and the entry stays his
            // (2_design.md §11.1).
            entry.quote = Some(candidate.quote.clone());
            entry.content.clone_from(statement);
            entry.stated_by = Some(Author::session_pass());
        }
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
        if let Some(instruction) = &self.instruction {
            // Approved by an agent on Daniel's checked instruction
            // (2_design.md §12.3): the entry stays whoever's words it holds,
            // and records who approved it, on which instruction, and the
            // command run. Rebuilt from the proposal, so a recovery writes
            // the same attribution as the approval.
            let agent = self
                .settled_author
                .clone()
                .filter(|a| a.kind == AuthorKind::Agent)?;
            entry.said = instructed_said(self.id, &agent, instruction);
            entry.approved_by = Some(agent);
            entry.instruction = Some(instruction.clone());
        }
        Some(entry)
    }
}

/// How the `said` of an entry approved on Daniel's instruction begins: the
/// canonical `proposal settle` command, where a typed approval's begins
/// `#scribe approve p-` (2_design.md §12.3).
pub(crate) const INSTRUCTED_SAID_PREFIX: &str = "scribe proposal settle p-";

/// The canonical command an agent ran to approve `id` on `instruction`:
/// `scribe proposal settle <id> --approve --author <name> --session <id>
/// --instruction '<his words>'`, the words shell-quoted as stored (redacted),
/// and left out once forgotten.
fn instructed_said(id: ProposalId, agent: &Author, instruction: &Instruction) -> String {
    let mut said = format!(
        "scribe proposal settle {id} --approve --author {} --session {} --instruction",
        agent.name, instruction.session
    );
    if let Some(quote) = &instruction.quote {
        said.push_str(" '");
        said.push_str(&quote.replace('\'', "'\\''"));
        said.push('\'');
    }
    said
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

/// Close every open candidate proposal whose tag is absent from `tags`
/// (2_design.md §11.3): `state` becomes `rejected`, `settled_at` becomes
/// `now`, `settled_by` stays `None`, `settled_author` becomes
/// `{kind: scribe, name: tag-retired}`, and `closed_reason` becomes
/// `Some(ClosedReason::TagRetired)`. Returns copies of the proposals closed,
/// in order; open candidates under tags in `tags` and settled or
/// non-candidate proposals are untouched.
pub fn close_retired_tag_candidates(
    proposals: &mut [Proposal],
    tags: &TagTable,
    now: OffsetDateTime,
) -> Vec<Proposal> {
    let mut closed = Vec::new();
    for proposal in proposals.iter_mut() {
        let Some(candidate) = &proposal.candidate else {
            continue;
        };
        if proposal.kind != ProposalKind::Candidate
            || proposal.state != ProposalState::Open
            || tags.get(&candidate.tag).is_some()
        {
            continue;
        }
        proposal.state = ProposalState::Rejected;
        proposal.settled_at = Some(now);
        proposal.settled_by = None;
        proposal.settled_author = Some(Author::tag_retired());
        proposal.closed_reason = Some(ClosedReason::TagRetired);
        closed.push(proposal.clone());
    }
    closed
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
