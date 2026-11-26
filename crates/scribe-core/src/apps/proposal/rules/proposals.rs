//! The proposal rules that came from the domain's `proposal` module
//! (ORB-27921 PR 5-15): whether a claim's context stands alone, the entry an
//! approved candidate becomes, closing candidates whose tag retired, and
//! which proposals a filter selects. The shapes are the store's contracts.

use crate::ContentFrom;
use crate::entry::{Entry, EntryId, Instruction, NewEntry, Speaker};
use crate::proposal::{
    CONTEXT_CHARS, CONTEXT_SENTENCES, ClosedReason, Proposal, ProposalId, ProposalKind,
    ProposalState,
};
use crate::record::{Author, AuthorKind, DateRange, Topics};
use crate::tag::TagTable;
use time::OffsetDateTime;

/// Whether this settlement authorizes materializing a candidate entry.
pub(in crate::apps::proposal) fn is_approved_candidate(proposal: &Proposal) -> bool {
    proposal.kind == ProposalKind::Candidate && proposal.state == ProposalState::Approved
}

/// The entry an approved candidate becomes (`Proposal::candidate_entry`).
pub trait CandidateEntry {
    /// Reconstruct an approved candidate's entry, including after an
    /// interrupted approval. The reserved id makes recovery idempotent.
    #[must_use]
    fn candidate_entry(&self, review_after: Option<time::Date>) -> Option<Entry>;
}

/// Whether `context` is a paragraph that can stand alone: non-empty, at
/// most [`CONTEXT_CHARS`] characters and at least [`CONTEXT_SENTENCES`]
/// sentences (2_design.md §15.3). Counted on the model's words, before
/// redaction.
#[must_use]
pub fn context_stands_alone(context: &str) -> bool {
    let context = context.trim();
    !context.is_empty()
        && context.chars().count() <= CONTEXT_CHARS
        && sentences(context) >= CONTEXT_SENTENCES
}

/// How many sentences `text` has: runs of `.`, `!` or `?` (closing quotes
/// and brackets after them allowed) that end the text or come before
/// whitespace.
fn sentences(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut count = 0;
    let mut i = 0;
    while i < chars.len() {
        if matches!(chars[i], '.' | '!' | '?') {
            let mut end = i;
            while end + 1 < chars.len()
                && matches!(
                    chars[end + 1],
                    '.' | '!' | '?' | '"' | '\'' | ')' | '”' | '’'
                )
            {
                end += 1;
            }
            if chars.get(end + 1).is_none_or(|c| c.is_whitespace()) {
                count += 1;
            }
            i = end + 1;
        } else {
            i += 1;
        }
    }
    count
}

impl CandidateEntry for Proposal {
    fn candidate_entry(&self, review_after: Option<time::Date>) -> Option<Entry> {
        if !is_approved_candidate(self) {
            return None;
        }
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
        // Scribe's context of the turn (or, for an older user claim, its
        // statement) is the content; the verbatim words stay in `quote`,
        // and the entry stays the speaker's (2_design.md §11.1, §15.3). An
        // older assistant claim keeps the assistant's words as content.
        let stated = match candidate.role {
            Speaker::User => candidate.stated(),
            Speaker::Assistant => candidate.context.as_deref(),
        };
        if let Some(stated) = stated {
            entry.quote = Some(candidate.quote.clone());
            entry.content = stated.to_owned();
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
