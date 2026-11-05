//! The consistency routine and the settling verbs
//! (docs/design/scribe/2_design.md §3, §7): which entries may disagree,
//! what the consistency model's labels come to, and what `#scribe approve`,
//! `reject`, `retire` and `forget` change.
//!
//! A scan, as `scribe consistency scan` runs it:
//!
//! 1. [`reapply`]: an approved proposal whose older entry is still `active`
//!    (a crash between the two writes of an approval) is applied again;
//! 2. [`candidates`], model-free: within each tag, pairs of active entries
//!    whose content tokens overlap (Jaccard at least [`JACCARD`]) or that
//!    share a topic, newest pairs first, at most [`MAX_PAIRS`]; and the
//!    active entries past `review_after`. A pair or a stale entry that
//!    already has a proposal, open or settled, is never offered again;
//! 3. the `consistency` model labels each pair, [`BATCH`] at a time
//!    ([`request`]);
//! 4. [`judge`] keeps a `contradicts` or `refines` label only when both its
//!    quotes pass the verbatim check, and turns it into a `supersedes`
//!    [`Draft`]; each stale entry becomes a `stale` draft ([`stale`]). The
//!    store numbers and writes them; nothing changes on an entry yet.
//!
//! The order of a pair is fixed here, never by the model: the newer entry
//! (by `created`, then id) supersedes the older.
//!
//! Settling is a `#scribe` instruction in a typed turn and nothing else
//! (4_decisions.md, "Proposals are settled only by #scribe in a typed
//! turn"): [`settle`] applies one [`Settle`] to the loaded proposals and
//! entries in memory, and the store writes what changed.
//!
//! Pure functions over loaded records: no I/O (STD-02 R5).

use crate::entry::{Cite, Entry, EntryId, EntryStatus};
use crate::models::{
    CONSISTENCY_BUDGET, ConsistencyAnswer, ConsistencyRequest, PairItem, PairSide,
};
use crate::proposal::{Proposal, ProposalId, ProposalKind, ProposalState, Quote};
use crate::verbatim;
use std::collections::BTreeSet;
use std::fmt;
use time::{Date, OffsetDateTime};

/// The least token overlap (intersection over union) that makes two
/// entries a candidate pair, as a fraction: one in four.
pub const JACCARD: (usize, usize) = (1, 4);
/// The most pairs one scan offers the model (§7).
pub const MAX_PAIRS: usize = 40;
/// The pairs one model call labels (§8: 120 s per batch of 10).
pub const BATCH: usize = 10;
/// The longest reason a proposal keeps, in characters (§3).
pub const REASON_CHARS: usize = 300;
/// The longest quote of a stale entry a proposal keeps, in characters.
pub const STALE_QUOTE_CHARS: usize = 200;

/// Two active entries of one tag that may disagree, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    /// The entry recorded first: the one a `supersedes` proposal would
    /// supersede.
    pub older: Entry,
    /// The entry recorded later.
    pub newer: Entry,
}

/// What a scan offers the model, and the entries due for review.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Candidates {
    /// Candidate pairs, newest first, at most the cap.
    pub pairs: Vec<Pair>,
    /// Candidate pairs the cap left for a later scan.
    pub over_cap: usize,
    /// Active entries past `review_after`, oldest first.
    pub stale: Vec<Entry>,
}

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

/// The order of two entries: the one recorded first, then the other.
fn ordered<'a>(a: &'a Entry, b: &'a Entry) -> (&'a Entry, &'a Entry) {
    let key = |e: &Entry| (e.created, e.id.number());
    if key(a) <= key(b) { (a, b) } else { (b, a) }
}

/// The distinct content tokens of an entry, as `ask` ranks them.
fn token_set(entry: &Entry) -> BTreeSet<String> {
    crate::ask::tokens(&entry.content).into_iter().collect()
}

/// Whether two token sets overlap by at least [`JACCARD`].
fn overlaps(a: &BTreeSet<String>, b: &BTreeSet<String>) -> bool {
    let common = a.intersection(b).count();
    let union = a.union(b).count();
    let (num, den) = JACCARD;
    common > 0 && common.saturating_mul(den) >= union.saturating_mul(num)
}

/// The scan's candidates (§7 step 1): within each tag, active entry pairs
/// whose content tokens overlap or that share a topic, without a proposal
/// in any state, newest pair (by its newer entry, then its older) first and
/// at most `max_pairs`; and the active entries whose `review_after` is
/// `today` or earlier, without a stale proposal in any state.
#[must_use]
pub fn candidates(
    entries: &[Entry],
    proposals: &[Proposal],
    today: Date,
    max_pairs: usize,
) -> Candidates {
    let active: Vec<(&Entry, BTreeSet<String>)> = entries
        .iter()
        .filter(|e| e.status == EntryStatus::Active)
        .map(|e| (e, token_set(e)))
        .collect();
    let mut pairs = Vec::new();
    for (i, (a, a_tokens)) in active.iter().enumerate() {
        for (b, b_tokens) in active.iter().skip(i.saturating_add(1)) {
            if a.tag != b.tag {
                continue;
            }
            let shares_topic = a
                .topics
                .as_slice()
                .iter()
                .any(|t| b.topics.as_slice().contains(t));
            if !shares_topic && !overlaps(a_tokens, b_tokens) {
                continue;
            }
            let (older, newer) = ordered(a, b);
            let draft = Draft {
                kind: ProposalKind::Supersedes,
                older: older.id.clone(),
                newer: Some(newer.id.clone()),
                reason: String::new(),
                quotes: Vec::new(),
            };
            if proposals.iter().any(|p| same_subject(p, &draft)) {
                continue;
            }
            pairs.push(Pair {
                older: older.clone(),
                newer: newer.clone(),
            });
        }
    }
    let key = |p: &Pair| {
        (
            p.newer.created,
            p.newer.id.clone(),
            p.older.created,
            p.older.id.clone(),
        )
    };
    pairs.sort_by_key(|p| std::cmp::Reverse(key(p)));
    let over_cap = pairs.len().saturating_sub(max_pairs);
    pairs.truncate(max_pairs);

    let mut stale: Vec<Entry> = active
        .iter()
        .map(|(e, _)| *e)
        .filter(|e| e.review_after.is_some_and(|d| d <= today))
        .filter(|e| {
            !proposals
                .iter()
                .any(|p| p.kind == ProposalKind::Stale && p.older == e.id)
        })
        .cloned()
        .collect();
    stale.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    Candidates {
        pairs,
        over_cap,
        stale,
    }
}

/// The model request for one batch of pairs, numbered from 1.
#[must_use]
pub fn request(model: &str, pairs: &[Pair]) -> ConsistencyRequest {
    let side = |e: &Entry| PairSide {
        id: e.id.to_string(),
        date: e.created.date().to_string(),
        text: e.content.clone(),
    };
    ConsistencyRequest {
        model: model.to_owned(),
        budget: CONSISTENCY_BUDGET,
        pairs: pairs
            .iter()
            .zip(1u32..)
            .map(|(pair, n)| PairItem {
                pair: n.to_string(),
                tag: pair.older.tag.to_string(),
                older: side(&pair.older),
                newer: side(&pair.newer),
            })
            .collect(),
    }
}

/// What [`judge`] made of one answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Judged {
    /// One `supersedes` draft per surviving `contradicts` or `refines`
    /// label, in pair order.
    pub drafts: Vec<Draft>,
    /// Pairs labelled `unrelated`.
    pub unrelated: usize,
    /// Labels dropped: a quote not verbatim in its entry, an unknown pair
    /// or label, or a second label for the same pair.
    pub dropped: usize,
}

/// Check the model's labels for `pairs` (the batch the request numbered):
/// a `contradicts` or `refines` label survives only when `older_quote` is
/// verbatim in the older entry and `newer_quote` in the newer. The reason
/// is trimmed and cut to [`REASON_CHARS`]; the order stays the pair's.
#[must_use]
pub fn judge(answer: ConsistencyAnswer, pairs: &[Pair]) -> Judged {
    let mut out = Judged::default();
    let mut seen = BTreeSet::new();
    let mut found: Vec<(usize, Draft)> = Vec::new();
    for j in answer.judgements {
        let index = j
            .pair
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|i| *i < pairs.len());
        let Some(index) = index.filter(|i| seen.insert(*i)) else {
            out.dropped = out.dropped.saturating_add(1);
            continue;
        };
        let pair = &pairs[index];
        match j.label.trim() {
            "unrelated" => out.unrelated = out.unrelated.saturating_add(1),
            "contradicts" | "refines" => {
                let older_quote = j.older_quote.trim();
                let newer_quote = j.newer_quote.trim();
                if !verbatim::contains(&pair.older.content, older_quote)
                    || !verbatim::contains(&pair.newer.content, newer_quote)
                {
                    out.dropped = out.dropped.saturating_add(1);
                    continue;
                }
                found.push((
                    index,
                    Draft {
                        kind: ProposalKind::Supersedes,
                        older: pair.older.id.clone(),
                        newer: Some(pair.newer.id.clone()),
                        reason: cut(j.reason.trim(), REASON_CHARS),
                        quotes: vec![
                            Quote {
                                entry: pair.older.id.clone(),
                                text: older_quote.to_owned(),
                            },
                            Quote {
                                entry: pair.newer.id.clone(),
                                text: newer_quote.to_owned(),
                            },
                        ],
                    },
                ));
            }
            _ => out.dropped = out.dropped.saturating_add(1),
        }
    }
    found.sort_by_key(|(i, _)| *i);
    out.drafts = found.into_iter().map(|(_, d)| d).collect();
    out
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
        .filter(|p| p.state == ProposalState::Approved)
    {
        let Some(entry) = entries
            .iter_mut()
            .find(|e| e.id == proposal.older && e.status == EntryStatus::Active)
        else {
            continue;
        };
        let (status, by) = approved_status(proposal);
        entry.status = status;
        entry.superseded_by = by;
        entry.updated = now;
        out.push(Reapplied {
            proposal: proposal.id,
            entry: entry.id.clone(),
            status,
        });
    }
    out
}

/// A settling instruction (form 4): `#scribe approve|reject <p-id>` or
/// `#scribe retire|forget <entry-id>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settle {
    /// Apply a proposal: its older entry becomes `superseded` (or `retired`
    /// for a stale proposal).
    Approve(ProposalId),
    /// Close a proposal for good; its pair is never proposed again.
    Reject(ProposalId),
    /// Hide an entry from answers; it stays on disk.
    Retire(EntryId),
    /// Blank an entry's text for good; its id, cite and status stay.
    Forget(EntryId),
}

impl Settle {
    /// The id the instruction names, as typed.
    #[must_use]
    pub fn target(&self) -> String {
        match self {
            Self::Approve(id) | Self::Reject(id) => id.to_string(),
            Self::Retire(id) | Self::Forget(id) => id.to_string(),
        }
    }
}

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
///   and `content` blanked, and every proposal quote of it and every
///   proposal reason about it blanked too, so its words leave the store.
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
                    Settled::Retired(id.clone())
                }
            }
        }
        Settle::Forget(id) => {
            let Some(entry) = entries.iter_mut().find(|e| e.id == *id) else {
                return Settled::Refused(SettleRefusal::NoEntry(id.clone()));
            };
            if entry.status == EntryStatus::Forgotten {
                return Settled::Already(format!("{id} is already forgotten"));
            }
            entry.status = EntryStatus::Forgotten;
            entry.said.clear();
            entry.content.clear();
            entry.updated = now;
            for proposal in proposals.iter_mut() {
                let about = proposal.older == *id || proposal.newer.as_ref() == Some(id);
                if about {
                    proposal.reason.clear();
                }
                for quote in proposal.quotes.iter_mut().filter(|q| q.entry == *id) {
                    quote.text.clear();
                }
            }
            Settled::Forgotten(id.clone())
        }
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
    entry.status = status;
    entry.superseded_by.clone_from(&by);
    entry.updated = now;
    Settled::Approved {
        proposal: id,
        entry: entry.id.clone(),
        status,
        by,
    }
}
