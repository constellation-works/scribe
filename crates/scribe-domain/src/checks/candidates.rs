//! Checks and shapes for the session pass's claims as the store writes them
//! under its lock (docs/design/scribe/2_design.md §15.2, §15.3).

use crate::pass::{PassClaim, own_span};
use crate::{
    Author, ContentFrom, Entry, EntryCandidate, EntryId, EntryStatus, Outcome, OutcomeState,
    Proposal, ProposalKind, ProposalState, Topics,
};

/// Whether a new pass claim carries what 2_design.md §15.3 requires: a
/// context paragraph that stands alone and a non-empty subject, each within
/// its bound, checked on the unmasked text, and no one-line statement. An
/// empty subject opens no claim.
pub fn has_context_and_subject(candidate: &EntryCandidate) -> bool {
    candidate.statement.is_none()
        && candidate
            .context
            .as_deref()
            .is_some_and(crate::context_stands_alone)
        && candidate.subject.as_deref().is_some_and(|text| {
            !text.trim().is_empty() && text.chars().count() <= crate::SUBJECT_CHARS
        })
}

/// Whether the pass already wrote `draft`'s claim as `entry`: the same tag
/// and turn, and either the same words or an entry since forgotten (whose
/// words are gone, and which must not come back).
pub fn written_from(entry: &Entry, draft: &EntryCandidate) -> bool {
    entry.author == Author::session_pass()
        && entry.tag == draft.tag
        && entry.cite.as_ref().is_some_and(|cite| {
            cite.session == draft.cite.session && cite.message == draft.cite.message
        })
        && (entry.status == EntryStatus::Forgotten
            || entry.quote.as_deref() == Some(draft.quote.as_str()))
}

/// A written claim's outcome, checked again under the lock: `None` when
/// the claim carries none (it was drafted for a `propose` tag). A `parked`
/// reason must still be a verbatim span of Daniel's own words of its turn
/// and need no redaction; otherwise the idea is written `open`.
pub fn checked_outcome(claim: &PassClaim, mask: &dyn Fn(&str) -> String) -> Option<Outcome> {
    let outcome = claim.outcome.clone()?;
    match (outcome.state, &outcome.reason, &claim.outcome_turn) {
        (OutcomeState::Parked, Some(reason), Some(turn))
            if own_span(turn, reason) && *reason == mask(reason) =>
        {
            Some(outcome)
        }
        (OutcomeState::Open, None, _) => Some(outcome),
        (OutcomeState::Parked | OutcomeState::Open | OutcomeState::Task, ..) => {
            Some(Outcome::open())
        }
    }
}

/// The active entry the pass writes for a checked claim under a `write`
/// tag: scribe's context as content, the turn's span as `quote`, authored
/// and stated by the session pass, nothing typed (`said` empty).
pub fn pass_entry(
    id: EntryId,
    draft: &EntryCandidate,
    outcome: Outcome,
    at: time::OffsetDateTime,
    review_after: Option<time::Date>,
) -> Entry {
    Entry {
        id,
        tag: draft.tag.clone(),
        status: EntryStatus::Active,
        project: draft.project.clone(),
        topics: Topics::default(),
        related: Vec::new(),
        provider: draft.provider.clone(),
        host: draft.host.clone(),
        author: Author::session_pass(),
        updated_by: None,
        created: at,
        updated: at,
        inactive_since: None,
        said: String::new(),
        content_from: ContentFrom::Message {
            role: draft.role,
            message: draft.cite.message.clone().unwrap_or_default(),
            at: draft.cite.at,
        },
        cite: Some(draft.cite.clone()),
        retirement_reason: None,
        superseded_by: None,
        review_after,
        quote: Some(draft.quote.clone()),
        window: None,
        evidence: None,
        as_of: None,
        approved_by: None,
        instruction: None,
        removal_instruction: None,
        stated_by: Some(Author::session_pass()),
        handoff_mode: None,
        decision: None,
        item: None,
        outcome: Some(outcome),
        content: draft.context.clone().unwrap_or_default(),
    }
}

/// How many open claims `tag` holds: the count its `max_open` caps
/// (2_design.md §15.2).
pub fn open_under(proposals: &[Proposal], tag: &crate::TagName) -> usize {
    proposals
        .iter()
        .filter(|p| p.kind == ProposalKind::Candidate && p.state == ProposalState::Open)
        .filter(|p| p.candidate.as_ref().is_some_and(|c| &c.tag == tag))
        .count()
}
