//! The entry side of the settling verbs (docs/design/scribe/2_design.md
//! §3, §7): what `forget` changes, and the outcome every settling verb
//! reports. Pure functions over loaded records: no I/O (STD-02 R5).

use crate::entry::{Entry, EntryId, EntryStatus};
use crate::proposal::{Proposal, ProposalId, ProposalState};
use std::fmt;
use time::OffsetDateTime;

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

/// What [`settle`](crate::consistency::settle) did.
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
