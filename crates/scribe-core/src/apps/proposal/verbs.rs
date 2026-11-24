//! Proposal verbs: agent proposals, the settling instructions typed in a
//! turn, consistency proposals and the recovery of interrupted approvals
//! (2_design.md §11, §12).

use crate::apps::entry::{required_message, required_reason};
use crate::apps::tag::ConfigReads;
use crate::apps::tag::check_agent_author;
use crate::consistency::{
    self, CoveredCandidate, Draft, Reapplied, ReapplyReport, Settle, Settled,
};
use crate::entry::Cite;
use crate::pass::covering_claim;
use crate::store::refusal;
use crate::{
    Author, AuthorKind, AuthorName, EntryId, EntryStatus, Error, Proposal, ProposalId,
    ProposalKind, ProposalState, Redactor, Result, Store, Tx, WriteRefusal,
};
use crate::{SessionForget, SessionId, handoff};
use std::num::NonZeroU64;
use time::OffsetDateTime;

/// The proposal verbs, implemented for [`Store`].
pub trait ProposalVerbs {
    /// Resolve an exact proposal id or a unique prefix of at least four
    /// characters, as the write verbs do, without locking or writing.
    fn resolve_proposal(&self, input: &str) -> Result<Proposal>;

    /// Open an agent proposal against either author's active entry.
    #[allow(
        clippy::too_many_arguments,
        reason = "explicit proposal inputs at the store boundary"
    )]
    fn add_agent_proposal(
        &self,
        kind: ProposalKind,
        older: &str,
        newer: Option<&str>,
        reason: &str,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Proposal, bool)>;

    /// Change an open agent proposal's reason or settle an agent entry's
    /// proposal. With Daniel's checked `instruction` (2_design.md §12.1,
    /// §12.3), an approve or reject also settles a session-pass candidate
    /// (`settle_candidate_on_instruction`); nothing else widens: a
    /// proposal about his entry is still his to settle.
    #[allow(
        clippy::too_many_arguments,
        reason = "explicit settlement inputs at the store boundary"
    )]
    fn change_agent_proposal(
        &self,
        id: &str,
        change: crate::ProposalChange,
        reason: Option<&str>,
        instruction: Option<&crate::instruction::Instructed>,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Proposal, bool)>;

    /// Apply one settling instruction typed in the turn `cite`, under the
    /// store lock ([`consistency::settle`]). `proposals.json` is written
    /// first and then the entry, so a crash between them leaves an approved
    /// proposal the next scan re-applies; only what changed is written.
    fn settle(&self, action: &Settle, cite: &Cite, now: time::OffsetDateTime) -> Result<Settled>;

    /// Open one proposal per draft, under the store lock: a draft whose
    /// pair (or stale entry) already has a proposal in any state, or whose
    /// entries are no longer all `active`, is skipped
    /// ([`consistency::is_settled_or_gone`], checked again here because a
    /// settle may have run since the drafts were made). The rest are
    /// numbered `p-<n>` past the highest id, their free text masked by
    /// `redactor`, and `proposals.json` is replaced whole. Returns the
    /// proposals opened, by id; writes nothing when there are none.
    fn open_proposals(
        &self,
        drafts: Vec<Draft>,
        now: time::OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Vec<Proposal>>;

    /// Re-apply, under the store lock, every approved proposal whose older
    /// entry is still `active` ([`consistency::reapply`]), and create any
    /// missing entry from an approved candidate. An approval
    /// writes `proposals.json` and then the entry, and this repairs a crash
    /// between the two. Writes only the entries it changes. An approved
    /// candidate an active entry of the same tag and project already covers
    /// is closed as `rejected` instead, with no new entry; that close is
    /// [`ReapplyReport::covered`] and the covering id is not stored on the
    /// proposal.
    fn reapply_approved(&self, now: time::OffsetDateTime) -> Result<ReapplyReport>;

    /// Close every open candidate proposal whose tag is absent from the
    /// effective tag table (`config.toml`, shipped tags, and `tags.json`),
    /// under the store lock (2_design.md §11.3). Returns the closed proposals
    /// in id order; writes nothing when none qualify.
    fn close_retired_candidates(&self, now: time::OffsetDateTime) -> Result<Vec<Proposal>>;
}

impl ProposalVerbs for Store {
    fn resolve_proposal(&self, input: &str) -> Result<Proposal> {
        self.read().proposals().resolve(input)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "explicit proposal inputs at the store boundary"
    )]
    fn add_agent_proposal(
        &self,
        kind: ProposalKind,
        older: &str,
        newer: Option<&str>,
        reason: &str,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Proposal, bool)> {
        check_agent_author(&author)?;
        required_reason(reason)?;
        self.write(|tx| {
        let older = tx.entries().resolve(older)?;
        let newer = newer.map(|id| tx.entries().resolve(id)).transpose()?;
        if kind == ProposalKind::Candidate || (kind == ProposalKind::Supersedes) != newer.is_some()
        {
            return Err(refusal(
                WriteRefusal::ProposalInvalid,
                "supersedes requires --newer; stale forbids it",
            ));
        }
        if newer.as_ref().is_some_and(|e| e.id == older.id) {
            return Err(refusal(
                WriteRefusal::ProposalInvalid,
                "older and newer must differ",
            ));
        }
        if older.status != EntryStatus::Active
            || newer
                .as_ref()
                .is_some_and(|e| e.status != EntryStatus::Active)
        {
            return Err(refusal(
                WriteRefusal::EntryNotActive,
                "proposal entries must be active",
            ));
        }
        let mut proposals = tx.proposals().all()?;
        let newer = newer.map(|e| e.id);
        if let Some(p) = proposals
            .iter()
            .find(|p| p.kind == kind && p.older == older.id && p.newer == newer)
        {
            if p.state == ProposalState::Open
                && p.reason == redactor.mask(reason)
                && p.author == Author::agent(author.clone())
            {
                return Ok((p.clone(), false));
            }
            return Err(refusal(
                WriteRefusal::ProposalInvalid,
                "a proposal already covers these entries; use proposal update on an open agent proposal",
            ));
        }
        let n = proposals
            .iter()
            .map(|p| p.id.number())
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| refusal(WriteRefusal::ProposalInvalid, "proposal ids exhausted"))?;
        let id: ProposalId = format!("p-{n}")
            .parse()
            .map_err(|e: crate::ProposalError| {
                refusal(WriteRefusal::ProposalInvalid, e.to_string())
            })?;
        let proposal = Proposal {
            id,
            kind,
            older: older.id,
            newer,
            reason: redactor.mask(reason),
            quotes: Vec::new(),
            state: ProposalState::Open,
            opened_at: now,
            settled_at: None,
            settled_by: None,
            author: Author::agent(author),
            updated_by: None,
            settled_author: None,
            closed_reason: None,
            instruction: None,
            candidate: None,
        };
        proposals.push(proposal.clone());
        tx.proposals().put_all(&proposals)?;
        Ok((proposal, true))
            })
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "explicit settlement inputs at the store boundary"
    )]
    fn change_agent_proposal(
        &self,
        id: &str,
        change: crate::ProposalChange,
        reason: Option<&str>,
        instruction: Option<&crate::instruction::Instructed>,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Proposal, bool)> {
        check_agent_author(&author)?;
        if let Some(reason) = reason {
            required_reason(reason)?;
        }
        self.write(|tx| {
        let mut proposals = tx.proposals().all()?;
        let ids: Vec<String> = proposals.iter().map(|p| p.id.to_string()).collect();
        let i = tx.proposals().position(id, &ids)?;
        let p = &mut proposals[i];
        if p.kind == ProposalKind::Candidate {
            let Some(instructed) = instruction.filter(|_| change != crate::ProposalChange::Update)
            else {
                // Without an instruction the refusal reads as it always has.
                return Err(refusal(
                    WriteRefusal::UserOnly,
                    if instruction.is_some() {
                        "The user settles candidate proposals through #scribe approve or reject; their reason is scribe's"
                    } else {
                        "The user settles candidate proposals through #scribe approve or reject"
                    },
                ));
            };
            let approve = change == crate::ProposalChange::Approve;
            return settle_candidate_on_instruction(tx,
                proposals, i, approve, reason, instructed, author, now, redactor,
            );
        }
        let before = p.clone();
        let reason_changed = reason.is_some_and(|r| redactor.mask(r) != p.reason);
        let mut changed_entry = None;
        if change != crate::ProposalChange::Update {
            let approve = change == crate::ProposalChange::Approve;
            let mut older = tx.entries().get(&p.older)?;
            if older.author.kind != AuthorKind::Agent {
                return Err(refusal(
                    WriteRefusal::UserOnly,
                    "The user settles proposals about their entries through #scribe approve or reject",
                ));
            }
            let state = if approve {
                ProposalState::Approved
            } else {
                ProposalState::Rejected
            };
            if p.state == state {
                if reason.is_some_and(|r| redactor.mask(r) != p.reason) {
                    return Err(refusal(
                        WriteRefusal::ProposalSettled,
                        "a settled proposal's reason cannot be changed",
                    ));
                }
                return Ok((p.clone(), false));
            }
            if p.state != ProposalState::Open {
                return Err(refusal(
                    WriteRefusal::ProposalSettled,
                    "proposal is already settled",
                ));
            }
            // The settler's `--reason` replaces the proposal's reason below,
            // so it may only do so on an agent-opened proposal: a reason
            // Daniel or scribe wrote stays as written, and settling without
            // `--reason` (or repeating theirs) keeps it.
            if reason_changed && p.author.kind != AuthorKind::Agent {
                return Err(refusal(
                    WriteRefusal::ProposalNotAgent,
                    "only an agent-opened proposal's reason can be changed; settle without --reason to keep the original reason",
                ));
            }
            if approve {
                if older.status != EntryStatus::Active {
                    return Err(refusal(
                        WriteRefusal::EntryNotActive,
                        "older entry is no longer active",
                    ));
                }
                if reason_changed {
                    p.updated_by = Some(Author::agent(author.clone()));
                }
                p.settled_author = Some(Author::agent(author.clone()));
                crate::consistency::apply_approval(p, &mut older, now);
                changed_entry = Some(older);
            } else if reason_changed {
                p.updated_by = Some(Author::agent(author.clone()));
            }
            p.state = state;
            p.settled_at = Some(now);
            p.settled_by = None;
            p.settled_author = Some(Author::agent(author));
        } else {
            if p.state != ProposalState::Open {
                return Err(refusal(
                    WriteRefusal::ProposalSettled,
                    "only open proposals can be updated",
                ));
            }
            if p.author.kind != AuthorKind::Agent {
                return Err(refusal(
                    WriteRefusal::ProposalNotAgent,
                    "only agent-opened proposals can be updated",
                ));
            }
            // Preserve the creator while recording who revised the reason.
            if reason_changed {
                p.updated_by = Some(Author::agent(author));
            }
        }
        if let Some(reason) = reason {
            p.reason = redactor.mask(reason);
        }
        let result = p.clone();
        let changed = result != before;
        if changed {
            tx.proposals().put_all(&proposals)?;
        }
        if let Some(entry) = changed_entry {
            tx.entries().put(&entry, false)?;
        }
        Ok((result, changed))
            })
    }

    fn settle(&self, action: &Settle, cite: &Cite, now: time::OffsetDateTime) -> Result<Settled> {
        self.write(|tx| {
            if let Settle::ForgetSession(id) = action {
                return Ok(match forget_session(tx, id, now)? {
                    SessionForgetOutcome::Forgotten(forgot) => Settled::SessionForgotten(forgot),
                    SessionForgetOutcome::Already {
                        id: resolved,
                        number,
                        cleaned: true,
                    } => Settled::SessionForgetRepaired {
                        id: resolved,
                        number,
                    },
                    SessionForgetOutcome::Already {
                        id: resolved,
                        cleaned: false,
                        ..
                    } => Settled::Already(format!("{resolved} is already forgotten")),
                });
            }
            let mut proposals = tx.proposals().all()?;
            let (mut entries, _) = tx.entries().all()?;
            let before_proposals = proposals.clone();
            let before_entries = entries.clone();
            let mut settled = consistency::settle(action, &mut proposals, &mut entries, cite, now);
            if let (Settle::Forget(id), Settled::Forgotten(_)) = (action, &settled)
                && let Some(entry) = before_entries.iter().find(|e| &e.id == id)
            {
                // The stored messages first (2_design.md §15.7): they hold the
                // words the forget removes, and deleting them again is a no-op;
                // the archive tables are erased in the same write.
                tx.entries().erase_residue(entry, &before_proposals, now)?;
            }
            if proposals != before_proposals {
                tx.proposals().put_all(&proposals)?;
            }
            if let Settle::Approve(id) = action
                && let Some(index) = proposals.iter().position(|p| {
                    p.id == *id
                        && p.kind == crate::ProposalKind::Candidate
                        && p.state == crate::ProposalState::Approved
                })
            {
                let proposal = proposals[index].clone();
                match write_candidate_entry(tx, &proposal)? {
                    CandidateWrite::Created => {
                        settled = Settled::Recorded {
                            proposal: *id,
                            entry: proposal.older.clone(),
                            agent_authored: proposal
                                .candidate
                                .as_ref()
                                .is_some_and(|c| c.role == crate::Speaker::Assistant),
                        };
                    }
                    CandidateWrite::Present => {}
                    CandidateWrite::Covered { by } => {
                        // Approved, but an active entry now says it: closed as
                        // covered, keeping who approved it and on what
                        // instruction, never as a rejection (format 15).
                        proposals[index].state = crate::ProposalState::Rejected;
                        proposals[index].closed_reason = Some(crate::ClosedReason::Covered);
                        tx.proposals().put_all(&proposals)?;
                        settled = Settled::Covered { proposal: *id, by };
                    }
                }
            }
            for (entry, before) in entries.iter().zip(&before_entries) {
                if entry != before {
                    tx.entries().put(entry, false)?;
                }
            }
            Ok(settled)
        })
    }

    fn open_proposals(
        &self,
        drafts: Vec<Draft>,
        now: time::OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Vec<Proposal>> {
        if drafts.is_empty() {
            return Ok(Vec::new());
        }
        self.write(|tx| {
            let mut proposals = tx.proposals().all()?;
            let (entries, _) = tx.entries().all()?;
            let mut next = proposals.iter().map(|p| p.id.number()).max().unwrap_or(0);
            let mut opened = Vec::new();
            for draft in drafts {
                if consistency::is_settled_or_gone(&draft, &proposals, &entries) {
                    continue;
                }
                next = next.saturating_add(1);
                let Ok(id) = ProposalId::try_from(format!("p-{next}")) else {
                    continue;
                };
                let mut proposal = draft.into_proposal(id, now);
                redactor.proposal(&mut proposal);
                proposals.push(proposal.clone());
                opened.push(proposal);
            }
            if !opened.is_empty() {
                tx.proposals().put_all(&proposals)?;
            }
            Ok(opened)
        })
    }

    fn reapply_approved(&self, now: time::OffsetDateTime) -> Result<ReapplyReport> {
        if !self.read().exists()? {
            return Ok(ReapplyReport::default());
        }
        let mut report = reapply_candidates(self)?;
        let proposals = self.read().proposals().all()?;
        let (entries, _) = self.read().entries().all()?;
        let mut probe = entries;
        if consistency::reapply(&proposals, &mut probe, now).is_empty() {
            return Ok(report);
        }
        self.write(|tx| {
            let proposals = tx.proposals().all()?;
            let (mut entries, _) = tx.entries().all()?;
            let done = consistency::reapply(&proposals, &mut entries, now);
            for change in &done {
                if let Some(entry) = entries.iter().find(|e| e.id == change.entry) {
                    tx.entries().put(entry, false)?;
                }
            }
            report.applied.extend(done);
            Ok(report)
        })
    }

    fn close_retired_candidates(&self, now: time::OffsetDateTime) -> Result<Vec<Proposal>> {
        if !self.read().exists()? {
            return Ok(Vec::new());
        }
        let config = self.read().tags().config()?;
        let mut probe = self.read().proposals().all()?;
        if crate::proposal::close_retired_tag_candidates(&mut probe, &config.tags, now).is_empty() {
            return Ok(Vec::new());
        }
        self.write(|tx| {
            let config = tx.tags().config()?;
            let mut proposals = tx.proposals().all()?;
            let closed =
                crate::proposal::close_retired_tag_candidates(&mut proposals, &config.tags, now);
            if !closed.is_empty() {
                tx.proposals().put_all(&proposals)?;
            }
            Ok(closed)
        })
    }
}

/// Settle candidate `proposals[i]` on Daniel's checked `instructed`
/// (2_design.md §12.3). The caller holds the lock. The candidate must be
/// open; the same instruction again changes nothing. An approval writes
/// the reserved entry as a typed approval does (his for a claim from his
/// turn, the assistant's for one from an assistant turn), with
/// `approved_by` the agent; a claim an active entry already covers is
/// closed `rejected` with `closed_reason: covered` and writes none. A
/// rejection writes no entry and sets `closed_reason: instructed`. Either way the proposal records the agent
/// in `settled_author`, the instruction turn's cite in `settled_by` and
/// `instruction` with its quote redacted. `proposals.json` is written
/// before the entry, so a crash between them leaves an approved proposal
/// the next scan re-applies with the same attribution.
#[allow(
    clippy::too_many_arguments,
    reason = "explicit settlement inputs at the store boundary"
)]
fn settle_candidate_on_instruction(
    tx: &Tx<'_>,
    mut proposals: Vec<Proposal>,
    i: usize,
    approve: bool,
    reason: Option<&str>,
    instructed: &crate::instruction::Instructed,
    author: AuthorName,
    now: OffsetDateTime,
    redactor: &Redactor,
) -> Result<(Proposal, bool)> {
    let p = &mut proposals[i];
    // A candidate's reason is scribe's: an agent never replaces it.
    if reason.is_some_and(|r| redactor.mask(r) != p.reason) {
        return Err(refusal(
            WriteRefusal::ProposalNotAgent,
            "only an agent-opened proposal's reason can be changed; settle without --reason to keep the original reason",
        ));
    }
    let same_instruction = p.instruction.as_ref().is_some_and(|held| {
        held.session == instructed.session && held.message == instructed.message
    });
    required_message(instructed)?;
    if p.state != ProposalState::Open {
        // The same command again, or an approval an active entry was
        // found to cover, is done; any other settlement stands.
        let done = if approve {
            p.state == ProposalState::Approved
                || p.closed_reason == Some(crate::ClosedReason::Covered)
        } else {
            p.closed_reason == Some(crate::ClosedReason::Instructed)
        };
        if same_instruction && done {
            return Ok((p.clone(), false));
        }
        return Err(refusal(
            WriteRefusal::ProposalSettled,
            format!("{} is already {}", p.id, p.state.as_str()),
        ));
    }
    // An instruction cannot settle a claim opened after he typed it
    // (2_design.md §12.2 check 3): he never saw it.
    if instructed.at < p.opened_at {
        return Err(refusal(
            WriteRefusal::InstructionStale,
            format!(
                "the instruction was typed before {} opened; ask the user again",
                p.id
            ),
        ));
    }
    p.state = if approve {
        ProposalState::Approved
    } else {
        ProposalState::Rejected
    };
    p.settled_at = Some(now);
    p.settled_by = Some(instructed.cite());
    p.settled_author = Some(Author::agent(author));
    p.instruction = Some(instructed.record(|q| redactor.mask(q)));
    if !approve {
        p.closed_reason = Some(crate::ClosedReason::Instructed);
        let result = p.clone();
        tx.proposals().put_all(&proposals)?;
        return Ok((result, true));
    }
    tx.proposals().put_all(&proposals)?;
    let snapshot = proposals[i].clone();
    if let CandidateWrite::Covered { .. } = write_candidate_entry(tx, &snapshot)? {
        // An active entry already says it: the approval is kept, with
        // its instruction, and closed as covered without an entry.
        proposals[i].state = ProposalState::Rejected;
        proposals[i].closed_reason = Some(crate::ClosedReason::Covered);
        tx.proposals().put_all(&proposals)?;
    }
    Ok((proposals[i].clone(), true))
}

/// Caller holds the lock. What happened to the reserved entry file.
fn write_candidate_entry(tx: &Tx<'_>, proposal: &Proposal) -> Result<CandidateWrite> {
    if proposal
        .candidate
        .as_ref()
        .is_some_and(|c| c.cite.is_deleted())
    {
        // The former source can no longer be used to create an entry.
        return Ok(CandidateWrite::Present);
    }
    let config = tx.tags().config()?;
    let review_after = config.tags.get(proposal.older.tag()).and_then(|tag| {
        proposal
            .settled_at
            .and_then(|at| tag.review_after(at.date()))
    });
    let entry = proposal.candidate_entry(review_after).ok_or_else(|| {
        Error::corrupt(tx.proposals().path(), "approved candidate lacks provenance")
    })?;
    match tx.entries().get(&entry.id) {
        // A previously completed approval keeps later retirement/forgetting.
        Ok(existing) => {
            if existing.author != entry.author || existing.cite != entry.cite {
                return Err(Error::corrupt(
                    tx.entries().path(&entry.id),
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
            tx.proposals().path(),
            "forgotten candidate cannot recreate a missing entry",
        ));
    }
    let (entries, _) = tx.entries().all()?;
    let candidate = proposal.candidate.as_ref().ok_or_else(|| {
        Error::corrupt(tx.proposals().path(), "approved candidate lacks provenance")
    })?;
    if let Some(cover) = covering_claim(&entries, candidate) {
        return Ok(CandidateWrite::Covered {
            by: cover.id.clone(),
        });
    }
    tx.entries().put(&entry, true)?;
    Ok(CandidateWrite::Created)
}

/// Approved proposals are the recovery journal, as for consistency approvals.
/// An approved candidate whose statement an active entry now covers is
/// closed as `rejected` with `closed_reason: covered`, keeping its
/// settlement and instruction, and reported in
/// [`ReapplyReport::covered`]; the covering id is not stored on the
/// proposal. An instructed approval is replayed with its agent and
/// instruction ([`Proposal::candidate_entry`]).
fn reapply_candidates(store: &Store) -> Result<ReapplyReport> {
    if !store
        .read()
        .proposals()
        .all()?
        .iter()
        .any(|p| p.kind == ProposalKind::Candidate && p.state == ProposalState::Approved)
    {
        return Ok(ReapplyReport::default());
    }
    store.write(|tx| {
        let mut proposals = tx.proposals().all()?;
        let mut report = ReapplyReport::default();
        let mut changed = false;
        for proposal in &mut proposals {
            if proposal.kind != ProposalKind::Candidate || proposal.state != ProposalState::Approved
            {
                continue;
            }
            let snapshot = proposal.clone();
            match write_candidate_entry(tx, &snapshot)? {
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
            tx.proposals().put_all(&proposals)?;
        }
        Ok(report)
    })
}

/// What [`write_candidate_entry`] did with a reserved candidate entry.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CandidateWrite {
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

/// Erase session data and detach dependent records in the write under way
/// ([`ProposalVerbs::settle`]). The tombstone also drives replay of
/// filesystem cleanup after commit.
fn forget_session(
    tx: &Tx<'_>,
    input: &SessionId,
    now: OffsetDateTime,
) -> Result<SessionForgetOutcome> {
    let id = match tx.sessions().resolve_or_handoff(input) {
        Ok(id) => id,
        Err(error @ Error::SessionNotFound { .. }) => {
            let known = tx.sessions().has_group_membership(input)?
                || tx
                    .entries()
                    .all()?
                    .0
                    .iter()
                    .any(|entry| handoff::session_ids(entry).contains(input));
            if !known {
                return Err(error);
            }
            input.clone()
        }
        Err(error) => return Err(error),
    };
    let number = tx.sessions().numbers()?.get(&id).copied();
    let erased = tx.sessions().erase(&id, number, now, || {
        crate::apps::entry::erase_session_documents(tx, &id, now)
    })?;
    let number = tx.sessions().numbers()?.get(&id).copied();
    Ok(if erased.already {
        SessionForgetOutcome::Already {
            id,
            number,
            cleaned: erased.changed || erased.files > 0,
        }
    } else {
        SessionForgetOutcome::Forgotten(SessionForget {
            id,
            number,
            messages: erased.messages,
            segments: erased.segments,
            files: erased.files,
            was_on: erased.was_on,
        })
    })
}

/// Result of forgetting a session, including cleanup on a replay.
pub enum SessionForgetOutcome {
    /// The session was forgotten now.
    Forgotten(SessionForget),
    /// The session was already forgotten.
    Already {
        /// The session's id.
        id: SessionId,
        /// Its number, while the store still holds one.
        number: Option<NonZeroU64>,
        /// Whether this replay cleaned up what the first forget left.
        cleaned: bool,
    },
}
