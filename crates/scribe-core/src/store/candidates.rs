//! Session-pass candidates, reservations and interrupted approval recovery.

use super::{Redactor, Store};
use crate::consistency::{CoveredCandidate, Reapplied, ReapplyReport};
use crate::pass::{
    MAX_CANDIDATES, MAX_COLLECTED, MAX_OPEN_CANDIDATES, admission, candidate_says, claim_covers,
    covering_claim, entry_says, own_span,
};
use crate::{
    Author, ContentFrom, Entry, EntryCandidate, EntryId, EntryStatus, Error, Outcome, OutcomeState,
    Proposal, ProposalId, ProposalKind, ProposalState, Result, SessionRecord, Topics,
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
                let Some(outcome) = checked_outcome(claim, redactor) else {
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
                self.write_entry(&entry, true)
                    .map_err(|err| after_entries(&written, err))?;
                written.push(entry.id.clone());
                entries.push(entry);
                continue;
            }
            if entries
                .iter()
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
            self.write_proposals(&proposals)
                .map_err(|err| after_entries(&written, err))?;
        }
        Ok(ClaimsWritten { closed, written })
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
        let candidate = proposal.candidate.as_ref().ok_or_else(|| {
            Error::corrupt(
                self.root.join(super::PROPOSALS_FILE),
                "approved candidate lacks provenance",
            )
        })?;
        if let Some(cover) = covering_claim(&entries, candidate) {
            return Ok(CandidateWrite::Covered {
                by: cover.id.clone(),
            });
        }
        self.write_entry(&entry, true)?;
        Ok(CandidateWrite::Created)
    }

    /// Approved proposals are the recovery journal, as for consistency approvals.
    /// An approved candidate whose statement an active entry now covers is
    /// closed as `rejected` with `closed_reason: covered`, keeping its
    /// settlement and instruction, and reported in
    /// [`ReapplyReport::covered`]; the covering id is not stored on the
    /// proposal. An instructed approval is replayed with its agent and
    /// instruction ([`Proposal::candidate_entry`]).
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
                    // The approval and its instruction stay; the close says
                    // why, so it never reads as a rejection (format 15).
                    proposal.state = ProposalState::Rejected;
                    proposal.closed_reason = Some(crate::ClosedReason::Covered);
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

/// Whether a new pass claim carries what 2_design.md §15.3 requires: a
/// context paragraph that stands alone and a non-empty subject, each within
/// its bound, checked on the unmasked text, and no one-line statement. An
/// empty subject opens no claim.
fn has_context_and_subject(candidate: &crate::EntryCandidate) -> bool {
    candidate.statement.is_none()
        && candidate
            .context
            .as_deref()
            .is_some_and(crate::context_stands_alone)
        && candidate.subject.as_deref().is_some_and(|text| {
            !text.trim().is_empty() && text.chars().count() <= crate::SUBJECT_CHARS
        })
}

/// Attach the ids of entries already written to `err`, so the command
/// still audits them; an empty list leaves `err` as it is.
fn after_entries(written: &[EntryId], err: Error) -> Error {
    if written.is_empty() {
        err
    } else {
        Error::AfterPersist(Box::new(crate::AfterPersistError {
            closed: written.iter().map(ToString::to_string).collect(),
            source: err,
        }))
    }
}

/// Whether the pass already wrote `draft`'s claim as `entry`: the same tag
/// and turn, and either the same words or an entry since forgotten (whose
/// words are gone, and which must not come back).
fn written_from(entry: &Entry, draft: &EntryCandidate) -> bool {
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
fn checked_outcome(claim: &crate::pass::PassClaim, redactor: &Redactor) -> Option<Outcome> {
    let outcome = claim.outcome.clone()?;
    match (outcome.state, &outcome.reason, &claim.outcome_turn) {
        (OutcomeState::Parked, Some(reason), Some(turn))
            if own_span(turn, reason) && *reason == redactor.mask(reason) =>
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
fn pass_entry(
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
        outcome: Some(outcome),
        content: draft.context.clone().unwrap_or_default(),
    }
}

/// How many open claims `tag` holds: the count its `max_open` caps
/// (2_design.md §15.2).
fn open_under(proposals: &[Proposal], tag: &crate::TagName) -> usize {
    proposals
        .iter()
        .filter(|p| p.kind == ProposalKind::Candidate && p.state == ProposalState::Open)
        .filter(|p| p.candidate.as_ref().is_some_and(|c| &c.tag == tag))
        .count()
}
