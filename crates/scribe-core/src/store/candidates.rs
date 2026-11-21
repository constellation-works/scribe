//! Session-pass candidates, reservations and interrupted approval recovery.

use super::{Redactor, Store};
use crate::consistency::Reapplied;
use crate::pass::{MAX_CANDIDATES, MAX_OPEN_CANDIDATES, admission, own_span, same_statement};
use crate::{
    Author, EntryId, EntryStatus, Error, Proposal, ProposalId, ProposalKind, ProposalState, Result,
    SessionRecord,
};

impl Store {
    /// Candidate ids are reserved without creating an entry file. Every entry
    /// allocator includes them, including rejected and approved proposals.
    pub(super) fn candidate_reserved_ids(&self) -> Result<Vec<EntryId>> {
        Ok(self
            .proposals()?
            .into_iter()
            .filter(|p| p.kind == ProposalKind::Candidate)
            .map(|p| p.older)
            .collect())
    }

    /// Caller holds the pass lock and has checked the record revision and seals.
    pub(super) fn open_pass_candidates(
        &self,
        record: &SessionRecord,
        drafts: &[crate::pass::PassClaim],
        redactor: &Redactor,
    ) -> Result<()> {
        if drafts.is_empty() || !self.control()?.get(&record.id).is_some_and(|c| c.on) {
            return Ok(());
        }
        let config = self.config()?;
        let (entries, skipped) = self.load_entries()?;
        if !skipped.is_empty() {
            // Without every entry, absence of an existing statement is unknown.
            tracing::warn!(
                skipped = skipped.len(),
                "not opening candidates: some entries are unreadable"
            );
            return Ok(());
        }
        let mut proposals = self.proposals()?;
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
        for claim in drafts.iter().take(MAX_CANDIDATES) {
            if open >= MAX_OPEN_CANDIDATES {
                break;
            }
            // The capture gate, repeated under the lock against the config
            // read now (2_design.md §11.2): a claim is Daniel's own words,
            // and a trigger of its tag fires in the turn it cites. The
            // admission persisted is the one computed here.
            let Some(admitted) = (claim.candidate.role == crate::Speaker::User
                && own_span(&claim.turn, &claim.candidate.quote))
            .then(|| admission(&config.tags, &claim.candidate.tag, &claim.turn))
            .flatten() else {
                continue;
            };
            let mut draft = claim.candidate.clone();
            draft.admitted = Some(admitted);
            let draft = &draft;
            if !draft.has_valid_provenance()
                || !(8..=400).contains(&draft.quote.chars().count())
                || draft.cite.session != record.id
                || draft.project != record.project
                || draft.quote != redactor.mask(&draft.quote)
                || draft.evidence.is_some()
                || entries
                    .iter()
                    .any(|entry| same_statement(&entry.content, &draft.quote))
                || proposals
                    .iter()
                    .filter_map(|p| p.candidate.as_ref())
                    .any(|prior| same_statement(&prior.quote, &draft.quote))
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
            };
            proposal.reason = proposal.reason.chars().take(300).collect();
            redactor.proposal(&mut proposal);
            proposals.push(proposal);
            open += 1;
            changed = true;
        }
        if changed {
            self.write_proposals(&proposals)?;
        }
        Ok(())
    }

    /// Caller holds the lock. True only when the reserved entry was created.
    pub(super) fn write_candidate_entry(&self, proposal: &Proposal) -> Result<bool> {
        let config = self.config()?;
        let review_after = config.tags.get(proposal.older.tag()).and_then(|tag| {
            proposal
                .settled_at
                .and_then(|at| tag.review_after(at.date()))
        });
        let entry = proposal.candidate_entry(review_after).ok_or_else(|| {
            Error::corrupt(
                self.root.join(super::PROPOSALS_FILE),
                "approved candidate lacks provenance",
            )
        })?;
        match self.entry(&entry.id) {
            // A previously completed approval keeps later retirement/forgetting.
            Ok(existing) => {
                if existing.author != entry.author || existing.cite != entry.cite {
                    return Err(Error::corrupt(
                        self.entry_path(&entry.id),
                        "reserved candidate id belongs to another entry",
                    ));
                }
                return Ok(false);
            }
            Err(Error::EntryNotFound { .. }) => {}
            Err(err) => return Err(err),
        }
        if entry.content.is_empty() {
            return Err(Error::corrupt(
                self.root.join(super::PROPOSALS_FILE),
                "forgotten candidate cannot recreate a missing entry",
            ));
        }
        self.write_entry(&entry, true)?;
        Ok(true)
    }

    /// Approved proposals are the recovery journal, as for consistency approvals.
    pub(super) fn reapply_candidates(&self) -> Result<Vec<Reapplied>> {
        if !self
            .proposals()?
            .iter()
            .any(|p| p.kind == ProposalKind::Candidate && p.state == ProposalState::Approved)
        {
            return Ok(Vec::new());
        }
        let _lock = self.lock("consistency scan")?;
        let mut done = Vec::new();
        for proposal in self
            .proposals()?
            .iter()
            .filter(|p| p.kind == ProposalKind::Candidate && p.state == ProposalState::Approved)
        {
            if self.write_candidate_entry(proposal)? {
                done.push(Reapplied {
                    proposal: proposal.id,
                    entry: proposal.older.clone(),
                    status: EntryStatus::Active,
                });
            }
        }
        Ok(done)
    }
}
