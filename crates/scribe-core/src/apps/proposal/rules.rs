//! The consistency routine and the settling verbs
//! (docs/design/scribe/2_design.md §3, §7): which entries are due for
//! review, and what `#scribe approve`,
//! `reject`, `retire` and `forget` change.
//!
//! A scan, as `scribe consistency scan` runs it, calls no model (ORB-27911
//! step 2a removed the `consistency` and `removal` judgements):
//!
//! 1. [`reapply`]: an approved proposal whose older entry is still `active`
//!    (a crash between the two writes of an approval) is applied again;
//! 2. [`stale_candidates`](crate::consistency::stale_candidates): the
//!    active entries past `review_after` with no stale proposal in any
//!    state, each a `stale` [`Draft`] ([`stale`](crate::consistency::stale)).
//!    The store numbers and writes them; nothing changes on an entry yet.
//!
//! After the entry steps a scan deactivates sessions by age
//! ([`crate::activity::past_age`]).
//!
//! Settling is a `#scribe` instruction in a typed turn and nothing else
//! (4_decisions.md, "Proposals are settled only by #scribe in a typed
//! turn"): [`settle`] applies one [`Settle`] to the loaded proposals and
//! entries in memory, and the store writes what changed.
//!
//! Pure functions over loaded records: no I/O (STD-02 R5).

use crate::apps::entry::{SettleRefusal, Settled, forget};
use crate::entry::{Cite, Entry, EntryId, EntryStatus};
use crate::proposal::{Proposal, ProposalId, ProposalKind, ProposalState, Quote};
use crate::record::Author;
use time::OffsetDateTime;

/// The longest reason a proposal keeps, in characters (§3).
pub const REASON_CHARS: usize = 300;

/// A proposal before the store numbers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// What it would change.
    pub kind: ProposalKind,
    /// The entry that would be superseded or retired.
    pub older: EntryId,
    /// The entry that supersedes it; `None` for a stale proposal.
    pub newer: Option<EntryId>,
    /// Why.
    pub reason: String,
    /// Verbatim spans of the entries involved.
    pub quotes: Vec<Quote>,
}

impl Draft {
    /// The stored proposal under `id`, opened at `now`.
    #[must_use]
    pub fn into_proposal(self, id: ProposalId, now: OffsetDateTime) -> Proposal {
        Proposal {
            id,
            kind: self.kind,
            older: self.older,
            newer: self.newer,
            reason: self.reason,
            quotes: self.quotes,
            state: ProposalState::Open,
            opened_at: now,
            settled_at: None,
            settled_by: None,
            author: Author::consistency(),
            updated_by: None,
            settled_author: None,
            closed_reason: None,
            instruction: None,
            candidate: None,
        }
    }
}

/// Whether `entries` and `proposals` already hold what `draft` proposes,
/// or `draft` no longer applies: a pair or stale entry with a proposal in
/// any state is never proposed again, and both entries must still be
/// active. The store checks this again under its lock.
#[must_use]
pub fn is_settled_or_gone(draft: &Draft, proposals: &[Proposal], entries: &[Entry]) -> bool {
    let active = |id: &EntryId| {
        entries
            .iter()
            .any(|e| &e.id == id && e.status == EntryStatus::Active)
    };
    let gone = !active(&draft.older) || draft.newer.as_ref().is_some_and(|n| !active(n));
    gone || proposals.iter().any(|p| same_subject(p, draft))
}

/// Whether `p` is about what `draft` is about: the same unordered pair for
/// `supersedes`, the same entry for `stale`.
fn same_subject(p: &Proposal, draft: &Draft) -> bool {
    if p.kind != draft.kind {
        return false;
    }
    match (&p.newer, &draft.newer) {
        (Some(a), Some(b)) => {
            (p.older == draft.older && a == b) || (&p.older == b && a == &draft.older)
        }
        (None, None) => p.older == draft.older,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

/// The status an approved proposal gives its older entry, and the entry
/// that replaced it.
#[must_use]
pub fn approved_status(proposal: &Proposal) -> (EntryStatus, Option<EntryId>) {
    match proposal.kind {
        ProposalKind::Supersedes => (EntryStatus::Superseded, proposal.newer.clone()),
        ProposalKind::Stale => (EntryStatus::Retired, None),
        ProposalKind::Candidate => (EntryStatus::Active, None),
    }
}

/// One approval applied again by [`reapply`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reapplied {
    /// The approved proposal.
    pub proposal: ProposalId,
    /// The older entry, now changed.
    pub entry: EntryId,
    /// Its status now.
    pub status: EntryStatus,
}

/// An approved candidate a recovery closed because an active entry already
/// covers its statement. The covering id is not stored on the proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoveredCandidate {
    /// The candidate proposal, now `rejected`.
    pub proposal: ProposalId,
    /// The active entry that covers the candidate's statement.
    pub by: EntryId,
}

/// What `ProposalVerbs::reapply_approved` in scribe-core repaired: approvals applied
/// again, and approved candidates closed because a later entry covers them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReapplyReport {
    /// Approvals whose older entry was still `active`.
    pub applied: Vec<Reapplied>,
    /// Approved candidates closed as `rejected` with no new entry.
    pub covered: Vec<CoveredCandidate>,
}

/// Apply every approved proposal whose older entry is still `active` to
/// `entries` again (§7: approval writes two files, and a crash between them
/// leaves the entry behind). Returns what changed, by proposal id.
pub fn reapply(
    proposals: &[Proposal],
    entries: &mut [Entry],
    now: OffsetDateTime,
) -> Vec<Reapplied> {
    let mut out = Vec::new();
    for proposal in proposals
        .iter()
        .filter(|p| p.state == ProposalState::Approved && p.kind != ProposalKind::Candidate)
    {
        let Some(entry) = entries
            .iter_mut()
            .find(|e| e.id == proposal.older && e.status == EntryStatus::Active)
        else {
            continue;
        };
        apply_approval(proposal, entry, now);
        let status = entry.status;
        out.push(Reapplied {
            proposal: proposal.id,
            entry: entry.id.clone(),
            status,
        });
    }
    out
}

pub use crate::capture::Settle;

/// Apply `action`, typed in the turn `cite`, to the loaded `proposals` and
/// `entries` in memory; the caller writes what changed (the proposals first,
/// so a crash leaves an approval the next scan re-applies).
///
/// - approve: an open proposal becomes `approved`, `settled_at` and
///   `settled_by` set, and its older entry, which must be `active`, becomes
///   `superseded` with `superseded_by` (or `retired` for a stale one);
/// - reject: an open proposal becomes `rejected`; its entries stay as they
///   are;
/// - retire: an `active` entry becomes `retired`;
/// - forget: any entry not yet forgotten becomes `forgotten`, its `said`
///   and `content` blanked, its `evidence`, `as_of`, `quote`, `window` and
///   `retirement_reason` cleared, and every proposal quote, candidate
///   `quote`, `evidence`, `admitted.matched` and `reason` about it cleared.
///   Session records remain.
///
/// The same instruction again changes nothing and says so.
pub fn settle(
    action: &Settle,
    proposals: &mut [Proposal],
    entries: &mut [Entry],
    cite: &Cite,
    now: OffsetDateTime,
) -> Settled {
    match action {
        Settle::Approve(id) => approve(*id, proposals, entries, cite, now),
        Settle::Reject(id) => {
            let Some(proposal) = proposals.iter_mut().find(|p| p.id == *id) else {
                return Settled::Refused(SettleRefusal::NoProposal(*id));
            };
            match proposal.state {
                ProposalState::Rejected => Settled::Already(format!("{id} is already rejected")),
                ProposalState::Approved => Settled::Refused(SettleRefusal::Settled {
                    proposal: *id,
                    state: ProposalState::Approved,
                }),
                ProposalState::Open => {
                    proposal.state = ProposalState::Rejected;
                    proposal.settled_at = Some(now);
                    proposal.settled_by = Some(cite.clone());
                    proposal.settled_author = Some(Author::user());
                    Settled::Rejected { proposal: *id }
                }
            }
        }
        Settle::Retire(id) => {
            let Some(entry) = entries.iter_mut().find(|e| e.id == *id) else {
                return Settled::Refused(SettleRefusal::NoEntry(id.clone()));
            };
            match entry.status {
                EntryStatus::Retired => Settled::Already(format!("{id} is already retired")),
                EntryStatus::Superseded | EntryStatus::Forgotten => {
                    Settled::Refused(SettleRefusal::NotActive {
                        entry: id.clone(),
                        status: entry.status,
                    })
                }
                EntryStatus::Active => {
                    entry.status = EntryStatus::Retired;
                    entry.updated = now;
                    entry.inactive_since = Some(now);
                    Settled::Retired(id.clone())
                }
            }
        }
        Settle::Forget(id) => forget(id, proposals, entries, now),
        // Sessions live in the store's database, not in these lists:
        // `ProposalVerbs::settle` applies it before it loads them.
        Settle::ForgetSession(id) => Settled::Already(format!(
            "{id} is a session; the store forgets it, not the proposal lists"
        )),
    }
}

fn approve(
    id: ProposalId,
    proposals: &mut [Proposal],
    entries: &mut [Entry],
    cite: &Cite,
    now: OffsetDateTime,
) -> Settled {
    let Some(proposal) = proposals.iter_mut().find(|p| p.id == id) else {
        return Settled::Refused(SettleRefusal::NoProposal(id));
    };
    match proposal.state {
        ProposalState::Approved => return Settled::Already(format!("{id} is already approved")),
        ProposalState::Rejected => {
            return Settled::Refused(SettleRefusal::Settled {
                proposal: id,
                state: ProposalState::Rejected,
            });
        }
        ProposalState::Open => {}
    }
    if proposal.kind == ProposalKind::Candidate {
        let covered_by = proposal.candidate.as_ref().and_then(|candidate| {
            crate::claim::covering_claim(entries, candidate).map(|entry| entry.id.clone())
        });
        if let Some(by) = covered_by {
            proposal.state = ProposalState::Rejected;
            proposal.settled_at = Some(now);
            proposal.settled_by = Some(cite.clone());
            proposal.settled_author = Some(Author::user());
            return Settled::Covered { proposal: id, by };
        }
        proposal.state = ProposalState::Approved;
        proposal.settled_at = Some(now);
        proposal.settled_by = Some(cite.clone());
        proposal.settled_author = Some(Author::user());
        return Settled::Recorded {
            proposal: id,
            entry: proposal.older.clone(),
            agent_authored: proposal
                .candidate
                .as_ref()
                .is_some_and(|c| c.role == crate::Speaker::Assistant),
        };
    }
    let Some(entry) = entries.iter_mut().find(|e| e.id == proposal.older) else {
        return Settled::Refused(SettleRefusal::NoEntry(proposal.older.clone()));
    };
    if entry.status != EntryStatus::Active {
        return Settled::Refused(SettleRefusal::NotActive {
            entry: entry.id.clone(),
            status: entry.status,
        });
    }
    let (status, by) = approved_status(proposal);
    proposal.state = ProposalState::Approved;
    proposal.settled_at = Some(now);
    proposal.settled_by = Some(cite.clone());
    proposal.settled_author = Some(Author::user());
    apply_approval(proposal, entry, now);
    Settled::Approved {
        proposal: id,
        entry: entry.id.clone(),
        status,
        by,
    }
}

/// The single entry effect used by human approval, agent approval and recovery.
pub fn apply_approval(proposal: &Proposal, entry: &mut Entry, now: OffsetDateTime) {
    let (status, by) = approved_status(proposal);
    entry.status = status;
    entry.superseded_by = by;
    entry.updated = now;
    entry.inactive_since = Some(now);
    if proposal
        .settled_author
        .as_ref()
        .is_some_and(|a| a.kind == crate::AuthorKind::Agent)
    {
        entry.updated_by.clone_from(&proposal.settled_author);
    }
}
