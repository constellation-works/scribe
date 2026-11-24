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
//! 2. [`stale_candidates`]: the active entries past `review_after` with no
//!    stale proposal in any state, each a `stale` [`Draft`] ([`stale`]).
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

use crate::entry::{Cite, Entry, EntryId, EntryStatus};
use crate::proposal::{Proposal, ProposalId, ProposalKind, ProposalState, Quote};
use crate::record::Author;
use crate::verbatim;
use std::fmt;
use time::{Date, OffsetDateTime};

/// The longest reason a proposal keeps, in characters (§3).
pub const REASON_CHARS: usize = 300;
/// The longest quote of a stale entry a proposal keeps, in characters.
pub const STALE_QUOTE_CHARS: usize = 200;

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

/// The scan's stale step (§7): the active entries whose `review_after` is
/// `today` or earlier, without a stale proposal in any state, oldest first.
#[must_use]
pub fn stale_candidates(entries: &[Entry], proposals: &[Proposal], today: Date) -> Vec<Entry> {
    let mut stale: Vec<Entry> = entries
        .iter()
        .filter(|e| e.status == EntryStatus::Active)
        .filter(|e| e.review_after.is_some_and(|d| d <= today))
        .filter(|e| {
            !proposals
                .iter()
                .any(|p| p.kind == ProposalKind::Stale && p.older == e.id)
        })
        .cloned()
        .collect();
    stale.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    stale
}

/// The `stale` draft for an entry past its `review_after`. The reason is
/// scribe's own sentence, not a model's; the quote is the start of the
/// entry's content, when it is long enough to pass the verbatim check.
#[must_use]
pub fn stale(entry: &Entry) -> Draft {
    let due = entry
        .review_after
        .map_or_else(String::new, |d| format!(" on {d}"));
    let start = cut(entry.content.trim(), STALE_QUOTE_CHARS);
    let quotes = if verbatim::contains(&entry.content, &start) {
        vec![Quote {
            entry: entry.id.clone(),
            text: start,
        }]
    } else {
        Vec::new()
    };
    Draft {
        kind: ProposalKind::Stale,
        older: entry.id.clone(),
        newer: None,
        reason: format!(
            "{} was due for review{due}; approve to retire it, reject to keep it",
            entry.id
        ),
        quotes,
    }
}

/// `text`'s first `max` characters.
fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
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

/// Why a settling instruction changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettleRefusal {
    /// No proposal has the id.
    NoProposal(ProposalId),
    /// No entry has the id.
    NoEntry(EntryId),
    /// The proposal was already settled the other way.
    Settled {
        /// The proposal.
        proposal: ProposalId,
        /// How it was settled.
        state: ProposalState,
    },
    /// The entry is not in a state the instruction applies to.
    NotActive {
        /// The entry.
        entry: EntryId,
        /// Where it stands.
        status: EntryStatus,
    },
}

impl SettleRefusal {
    /// The stable code the audit log and the reply carry (STD-01 R19).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoProposal(_) => "proposal_not_found",
            Self::NoEntry(_) => "entry_not_found",
            Self::Settled { .. } => "proposal_settled",
            Self::NotActive { .. } => "entry_not_active",
        }
    }
}

impl fmt::Display for SettleRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoProposal(id) => {
                write!(f, "no proposal {id}; scribe proposal list shows them")
            }
            Self::NoEntry(id) => write!(f, "no entry {id}; scribe entry list shows them"),
            Self::Settled { proposal, state } => {
                write!(f, "{proposal} is already {}", state.as_str())
            }
            Self::NotActive { entry, status } => write!(f, "{entry} is {status}, not active"),
        }
    }
}

/// What [`settle`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settled {
    /// An approved candidate creates a new entry: Daniel-authored for a
    /// candidate from his own turn, agent-authored for one from an
    /// assistant turn.
    Recorded {
        /// The approved candidate proposal.
        proposal: ProposalId,
        /// The new entry, under its reserved id.
        entry: EntryId,
        /// Whether the entry is the assistant's (`true`) or Daniel's.
        agent_authored: bool,
    },
    /// The proposal was approved and its older entry changed.
    Approved {
        /// The proposal.
        proposal: ProposalId,
        /// Its older entry.
        entry: EntryId,
        /// The entry's status now.
        status: EntryStatus,
        /// The entry that replaced it, for a `supersedes` proposal.
        by: Option<EntryId>,
    },
    /// The proposal was rejected.
    Rejected {
        /// The proposal.
        proposal: ProposalId,
    },
    /// The entry was retired.
    Retired(EntryId),
    /// The entry's text was blanked.
    Forgotten(EntryId),
    /// An open candidate's statement is already covered by an active entry
    /// of the same tag and project. The proposal is `rejected`, `newer`
    /// stays empty, and no entry is written. The covering id is named here
    /// for the reply only; it is not stored on the proposal.
    Covered {
        /// The candidate proposal.
        proposal: ProposalId,
        /// The active entry that covers the statement.
        by: EntryId,
    },
    /// A session's stored data was erased, leaving its id as a tombstone.
    SessionForgotten(crate::session::SessionForget),
    /// The session was already forgotten and the store had residue to erase.
    SessionForgetRepaired {
        /// The provider session id.
        id: crate::SessionId,
        /// The session's short number, if it has one.
        number: Option<std::num::NonZeroU64>,
    },
    /// It was already so (a replayed instruction): nothing changed.
    Already(String),
    /// It was refused: nothing changed.
    Refused(SettleRefusal),
}

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

/// Forget the entry `id`: any entry not yet forgotten becomes `forgotten`,
/// its `said` and `content` blanked, its `evidence`, `as_of`, `quote`,
/// `window` and `retirement_reason` cleared and its instruction's quote
/// dropped, and every proposal quote, candidate `quote`, `evidence`,
/// `context`, `statement`, `subject`, `admitted.matched`, instruction quote and
/// `reason` about it cleared.
/// Session records remain. The typed `#scribe forget` and the instructed
/// `entry forget` both apply it; an entry already forgotten is
/// [`Settled::Already`].
pub fn forget(
    id: &EntryId,
    proposals: &mut [Proposal],
    entries: &mut [Entry],
    now: OffsetDateTime,
) -> Settled {
    let Some(entry) = entries.iter_mut().find(|e| e.id == *id) else {
        return Settled::Refused(SettleRefusal::NoEntry(id.clone()));
    };
    if entry.status == EntryStatus::Forgotten {
        return Settled::Already(format!("{id} is already forgotten"));
    }
    entry.status = EntryStatus::Forgotten;
    entry.said.clear();
    entry.content.clear();
    entry.evidence = None;
    entry.as_of = None;
    entry.quote = None;
    entry.decision = None;
    entry.item = None;
    entry.window = None;
    entry.retirement_reason = None;
    for instruction in [&mut entry.instruction, &mut entry.removal_instruction]
        .into_iter()
        .flatten()
    {
        instruction.quote = None;
    }
    // A parked idea's reason is Daniel's words; its state stays.
    if let Some(outcome) = &mut entry.outcome {
        outcome.reason = None;
    }
    entry.updated = now;
    // Forgetting keeps the time a retired or superseded entry left
    // `active`; an active one leaves it now.
    entry.inactive_since.get_or_insert(now);
    forget_proposals(id, proposals);
    Settled::Forgotten(id.clone())
}

/// Blank every proposal quote, candidate payload, instruction quote and reason
/// about `id`. Returns whether any proposal changed.
pub(crate) fn forget_proposals(id: &EntryId, proposals: &mut [Proposal]) -> bool {
    let mut changed = false;
    for proposal in proposals.iter_mut() {
        let before = proposal.clone();
        let about = proposal.older == *id || proposal.newer.as_ref() == Some(id);
        if about {
            proposal.reason.clear();
            if let Some(candidate) = &mut proposal.candidate {
                candidate.quote.clear();
                candidate.context = None;
                candidate.statement = None;
                candidate.subject = None;
                candidate.evidence = None;
                if let Some(admitted) = &mut candidate.admitted {
                    admitted.matched.clear();
                }
            }
            if let Some(instruction) = &mut proposal.instruction {
                instruction.quote = None;
            }
        }
        for quote in proposal.quotes.iter_mut().filter(|q| q.entry == *id) {
            quote.text.clear();
        }
        if *proposal != before {
            changed = true;
        }
    }
    changed
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
