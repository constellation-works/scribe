//! Session-pass candidates, reservations and interrupted approval recovery.

use super::{Redactor, Store};
use crate::checks::candidates::{
    checked_outcome, has_context_and_subject, open_under, pass_entry, written_from,
};
use crate::pass::{
    MAX_CANDIDATES, MAX_COLLECTED, MAX_OPEN_CANDIDATES, admission, candidate_says, claim_covers,
    covering_claim, entry_says, own_span,
};
use crate::{
    Author, EntryId, Error, Proposal, ProposalId, ProposalKind, ProposalState, Result,
    SessionRecord,
};

/// What the candidate write path changed under the store lock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ClaimsWritten {
    /// Open candidates closed because their tag is retired, in id order.
    pub(super) closed: Vec<Proposal>,
    /// Entries the pass wrote itself under `write` tags, in write order.
    pub(super) written: Vec<EntryId>,
}

impl Store {
    /// Include reserved candidate ids and deleted handoff ids when
    /// allocating an entry id.
    pub(super) fn candidate_reserved_ids(&self) -> Result<Vec<EntryId>> {
        let mut ids: Vec<_> = self
            .proposals()?
            .into_iter()
            .filter(|p| p.kind == ProposalKind::Candidate)
            .map(|p| p.older)
            .collect();
        let deleted = self
            .read_db(super::db::docs::deleted_handoffs)?
            .unwrap_or_default();
        ids.extend(deleted);
        Ok(ids)
    }

    /// Caller holds the pass lock and has checked the record revision and seals.
    /// Closes any open candidates whose tag is no longer in the tag table
    /// before counting the open-candidate cap, and returns the closed
    /// proposals in id order.
    ///
    /// A claim under a `write` tag (2_design.md §11.6) is written here as an
    /// active entry the pass authors, with no proposal, at most
    /// [`MAX_COLLECTED`] per call and outside the open-candidate cap, once the
    /// same checks as a candidate pass and no active entry of its tag and
    /// project, no prior candidate and no entry the pass wrote from the same
    /// turn already says it.
    pub(super) fn open_pass_candidates(
        &self,
        record: &SessionRecord,
        drafts: &[crate::pass::PassClaim],
        redactor: &Redactor,
    ) -> Result<ClaimsWritten> {
        if !self.control()?.get(&record.id).is_some_and(|c| c.on) {
            return Ok(ClaimsWritten::default());
        }
        let config = self.config()?;
        let mut proposals = self.proposals()?;
        let closed = crate::proposal::close_retired_tag_candidates(
            &mut proposals,
            &config.tags,
            record.updated,
        );
        if drafts.is_empty() {
            if !closed.is_empty() {
                self.write_proposals(&proposals)?;
            }
            return Ok(ClaimsWritten {
                closed,
                written: Vec::new(),
            });
        }
        let (mut entries, skipped) = self.load_entries()?;
        if !skipped.is_empty() {
            if !closed.is_empty() {
                self.write_proposals(&proposals)?;
            }
            // Without every entry, absence of an existing statement is unknown.
            tracing::warn!(
                skipped = skipped.len(),
                "not opening candidates: some entries are unreadable"
            );
            return Ok(ClaimsWritten {
                closed,
                written: Vec::new(),
            });
        }
        let mut open = proposals
            .iter()
            .filter(|p| p.kind == ProposalKind::Candidate && p.state == ProposalState::Open)
            .count();
        let mut ids: Vec<_> = entries
            .iter()
            .map(|e| e.id.clone())
            .chain(
                proposals
                    .iter()
                    .filter(|p| p.kind == ProposalKind::Candidate)
                    .map(|p| p.older.clone()),
            )
            .collect();
        let mut next = proposals.iter().map(|p| p.id.number()).max().unwrap_or(0);
        let mut changed = false;
        let (mut proposed, mut written) = (0_usize, Vec::<EntryId>::new());
        for claim in drafts {
            let tag = config.tags.get(&claim.candidate.tag);
            let writes = tag.is_some_and(|tag| tag.capture.writes());
            if writes {
                // The bound on collected entries applies per tag (§15.2).
                let collected = written
                    .iter()
                    .filter(|id| id.tag() == &claim.candidate.tag)
                    .count();
                if collected >= MAX_COLLECTED {
                    continue;
                }
            } else if proposed >= MAX_CANDIDATES
                || open >= MAX_OPEN_CANDIDATES
                || open_under(&proposals, &claim.candidate.tag)
                    >= tag.map_or(0, |tag| {
                        usize::try_from(tag.capture.max_open()).unwrap_or(usize::MAX)
                    })
            {
                continue;
            }
            // The capture gate, repeated under the lock against the config
            // read now (2_design.md §11.2, §15.2): a claim is the own words of
            // a turn by a speaker its tag reads, and a trigger of the tag
            // fires in that turn. The admission persisted is the one
            // computed here.
            let Some(admitted) = own_span(&claim.turn, &claim.candidate.quote)
                .then(|| {
                    admission(
                        &config.tags,
                        &claim.candidate.tag,
                        claim.candidate.role,
                        &claim.turn,
                    )
                })
                .flatten()
            else {
                continue;
            };
            // The bounds hold the model's words as given; masking a secret
            // may lengthen them, and the masked value is kept.
            if !has_context_and_subject(&claim.candidate) {
                continue;
            }
            let mut draft = claim.candidate.clone();
            draft.admitted = Some(admitted);
            // The context and subject are the model's words: kept
            // redacted when redaction changes them. Only the quote must be
            // the speaker's words unchanged (2_design.md §15.3, §5.3).
            draft.context = draft.context.as_deref().map(|text| redactor.mask(text));
            draft.subject = draft.subject.as_deref().map(|text| redactor.mask(text));
            let draft = &draft;
            let says = candidate_says(draft);
            if !draft.has_valid_provenance()
                || !(8..=400).contains(&draft.quote.chars().count())
                || draft.cite.session != record.id
                || draft.project != record.project
                || draft.quote != redactor.mask(&draft.quote)
                || draft.evidence.is_some()
                || proposals
                    .iter()
                    .filter_map(|p| p.candidate.as_ref())
                    .any(|prior| claim_covers(&candidate_says(prior), &says))
            {
                continue;
            }
            if writes {
                // The shared duplicate check (`covering_claim`: active
                // entries of the tag and project), and any entry the pass
                // already wrote from this turn that is forgotten or holds
                // the same words, so a retried window cannot write it twice.
                if covering_claim(&entries, draft).is_some()
                    || entries.iter().any(|entry| written_from(entry, draft))
                {
                    continue;
                }
                let Some(outcome) = checked_outcome(claim, &|s| redactor.mask(s)) else {
                    continue;
                };
                let id = EntryId::next_in(&draft.tag, &ids);
                ids.push(id.clone());
                let review_after = config
                    .tags
                    .get(&draft.tag)
                    .and_then(|tag| tag.review_after(record.updated.date()));
                let mut entry = pass_entry(id, draft, outcome, record.updated, review_after);
                redactor.entry(&mut entry);
                self.write_entry(&entry, true)?;
                written.push(entry.id.clone());
                entries.push(entry);
                continue;
            }
            if entries
                .iter()
                .filter(|entry| entry.tag.as_str() != crate::handoff::HANDOFF_TAG)
                .any(|entry| claim_covers(&entry_says(entry), &says))
            {
                continue;
            }
            let Some(number) = next.checked_add(1) else {
                break;
            };
            next = number;
            let id: ProposalId = format!("p-{next}")
                .parse()
                .map_err(|e| Error::corrupt(self.root.join(super::PROPOSALS_FILE), e))?;
            let older = EntryId::next_in(&draft.tag, &ids);
            ids.push(older.clone());
            let mut proposal = Proposal {
                id,
                kind: ProposalKind::Candidate,
                older,
                newer: None,
                candidate: Some(draft.clone()),
                reason: format!("Unmarked {} statement: {}", draft.tag, draft.quote),
                quotes: Vec::new(),
                state: ProposalState::Open,
                opened_at: record.updated,
                settled_at: None,
                settled_by: None,
                author: Author::session_pass(),
                updated_by: None,
                settled_author: None,
                closed_reason: None,
                instruction: None,
            };
            proposal.reason = proposal.reason.chars().take(300).collect();
            redactor.proposal(&mut proposal);
            proposals.push(proposal);
            open += 1;
            proposed += 1;
            changed = true;
        }
        if !closed.is_empty() || changed {
            self.write_proposals(&proposals)?;
        }
        Ok(ClaimsWritten { closed, written })
    }
}
