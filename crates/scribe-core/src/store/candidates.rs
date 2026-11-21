//! Session-pass candidates, reservations and interrupted approval recovery.

use super::{Redactor, Store};
use crate::consistency::{CoveredCandidate, Reapplied, ReapplyReport};
use crate::pass::{
    MAX_CANDIDATES, MAX_OPEN_CANDIDATES, admission, covering_entry, covers_statement, own_span,
};
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
    /// Closes any open candidates whose tag is no longer in the tag table
    /// before counting the open-candidate cap, and returns the closed
    /// proposals in id order.
    pub(super) fn open_pass_candidates(
        &self,
        record: &SessionRecord,
        drafts: &[crate::pass::PassClaim],
        redactor: &Redactor,
    ) -> Result<Vec<Proposal>> {
        if !self.control()?.get(&record.id).is_some_and(|c| c.on) {
            return Ok(Vec::new());
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
            return Ok(closed);
        }
        let (entries, skipped) = self.load_entries()?;
        if !skipped.is_empty() {
            if !closed.is_empty() {
                self.write_proposals(&proposals)?;
            }
            // Without every entry, absence of an existing statement is unknown.
            tracing::warn!(
                skipped = skipped.len(),
                "not opening candidates: some entries are unreadable"
            );
            return Ok(closed);
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
                    .any(|entry| covers_statement(&entry.content, &draft.quote))
                || proposals
                    .iter()
                    .filter_map(|p| p.candidate.as_ref())
                    .any(|prior| covers_statement(&prior.quote, &draft.quote))
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
        if !closed.is_empty() || changed {
            self.write_proposals(&proposals)?;
        }
        Ok(closed)
    }

    /// Close every open candidate proposal whose tag is absent from the
    /// effective tag table (`config.toml`, shipped tags, and `tags.json`),
    /// under the store lock (2_design.md §11.3). Returns the closed proposals
    /// in id order; writes nothing when none qualify.
    pub fn close_retired_candidates(&self, now: time::OffsetDateTime) -> Result<Vec<Proposal>> {
        if !self.root_exists()? {
            return Ok(Vec::new());
        }
        let config = self.config()?;
        let mut probe = self.proposals()?;
        if crate::proposal::close_retired_tag_candidates(&mut probe, &config.tags, now).is_empty() {
            return Ok(Vec::new());
        }
        let _lock = self.lock("consistency scan")?;
        let config = self.config()?;
        let mut proposals = self.proposals()?;
        let closed =
            crate::proposal::close_retired_tag_candidates(&mut proposals, &config.tags, now);
        if !closed.is_empty() {
            self.write_proposals(&proposals)?;
        }
        Ok(closed)
    }

    /// Caller holds the lock. What happened to the reserved entry file.
    pub(super) fn write_candidate_entry(&self, proposal: &Proposal) -> Result<CandidateWrite> {
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
                return Ok(CandidateWrite::Present);
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
        let (entries, _) = self.load_entries()?;
        if let Some(cover) = covering_entry(&entries, &entry.content, &entry.tag, &entry.project) {
            return Ok(CandidateWrite::Covered {
                by: cover.id.clone(),
            });
        }
        self.write_entry(&entry, true)?;
        Ok(CandidateWrite::Created)
    }

    /// Approved proposals are the recovery journal, as for consistency approvals.
    /// An approved candidate whose statement an active entry now covers is
    /// closed as `rejected` and reported in [`ReapplyReport::covered`]; the
    /// covering id is not stored on the proposal.
    pub(super) fn reapply_candidates(&self) -> Result<ReapplyReport> {
        if !self
            .proposals()?
            .iter()
            .any(|p| p.kind == ProposalKind::Candidate && p.state == ProposalState::Approved)
        {
            return Ok(ReapplyReport::default());
        }
        let _lock = self.lock("consistency scan")?;
        let mut proposals = self.proposals()?;
        let mut report = ReapplyReport::default();
        let mut changed = false;
        for proposal in &mut proposals {
            if proposal.kind != ProposalKind::Candidate || proposal.state != ProposalState::Approved
            {
                continue;
            }
            let snapshot = proposal.clone();
            match self.write_candidate_entry(&snapshot)? {
                CandidateWrite::Created => report.applied.push(Reapplied {
                    proposal: snapshot.id,
                    entry: snapshot.older.clone(),
                    status: EntryStatus::Active,
                }),
                CandidateWrite::Present => {}
                CandidateWrite::Covered { by } => {
                    proposal.state = ProposalState::Rejected;
                    report.covered.push(CoveredCandidate {
                        proposal: snapshot.id,
                        by,
                    });
                    changed = true;
                }
            }
        }
        if changed {
            self.write_proposals(&proposals)?;
        }
        Ok(report)
    }
}

/// What [`Store::write_candidate_entry`] did with a reserved candidate entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CandidateWrite {
    /// The reserved entry file was created.
    Created,
    /// The reserved entry file was already present.
    Present,
    /// An active entry of the same tag and project already covers the
    /// statement, so no file was written.
    Covered {
        /// That entry.
        by: EntryId,
    },
}
