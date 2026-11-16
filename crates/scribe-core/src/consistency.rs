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
//! After the entry steps (S10) a scan deactivates sessions: the `age`
//! route ([`crate::activity::past_age`]) model-free, then the `removed`
//! route: [`removal_candidates`] pairs each active, unpinned session with
//! later Daniel-authored entries that carry a removal cue, the model
//! labels them ([`removal_request`]), and [`judge_removals`] keeps a
//! `removed` label only when its quote is verbatim in that entry.
//! Model-written session text (headers and segments, sealed or not) and
//! agent-written entries are never removal evidence: injected or
//! agent-supplied words must not be able to satisfy the quote check and
//! hide a session from `ask` (4_decisions.md, "Removal evidence is
//! Daniel-authored entries only").
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

use crate::activity::{self, ActiveRoute, SetActive};
use crate::entry::{Cite, Entry, EntryId, EntryStatus};
use crate::models::{
    CONSISTENCY_BUDGET, ConsistencyAnswer, ConsistencyRequest, PairItem, PairSide, RemovalAnswer,
    RemovalLater, RemovalPairItem, RemovalRequest, RemovalSession,
};
use crate::proposal::{Proposal, ProposalId, ProposalKind, ProposalState, Quote};
use crate::record::{Author, AuthorKind};
use crate::session::{SessionId, SessionRecord};
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
            author: Author::consistency(),
            updated_by: None,
            settled_author: None,
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
                    proposal.settled_author = Some(Author::daniel());
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
                    if let Some(candidate) = &mut proposal.candidate {
                        candidate.quote.clear();
                    }
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
    if proposal.kind == ProposalKind::Candidate {
        proposal.state = ProposalState::Approved;
        proposal.settled_at = Some(now);
        proposal.settled_by = Some(cite.clone());
        proposal.settled_author = Some(Author::daniel());
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
    proposal.settled_author = Some(Author::daniel());
    apply_approval(proposal, entry, now);
    Settled::Approved {
        proposal: id,
        entry: entry.id.clone(),
        status,
        by,
    }
}

/// The most removed-feature pairs one scan offers the model (§7, S10).
pub const MAX_REMOVAL_PAIRS: usize = 20;
/// How recently a Daniel-authored entry must have been written or
/// updated to be offered, in days: a weekly scan sees each new entry
/// once (§7).
pub const REMOVAL_RECENT_DAYS: i64 = 8;
/// The words that make a Daniel-authored entry a removal candidate (§7).
pub const REMOVAL_CUES: &[&str] = &[
    "removed",
    "deleted",
    "dropped",
    "decommissioned",
    "retired",
    "replaced",
    "superseded",
    "ripped out",
    "sunset",
];

/// A Daniel-authored entry written after a session that may say its
/// feature is gone. Only entries Daniel wrote (`author.kind` is
/// [`AuthorKind::Daniel`]) are removal evidence; session headers and
/// segments are model-written and agent entries are agent-written, so
/// neither can deactivate a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaterRecord {
    /// The entry id: what a deactivation cites.
    pub cite: String,
    /// The entry id again, so one session is paired with one unit of
    /// each record at most.
    pub record: String,
    /// The entry's `created`.
    pub date: OffsetDateTime,
    /// The text the model reads and the quote is checked against.
    pub text: String,
    /// Its topics.
    pub topics: Vec<String>,
}

/// An active, unpinned session and a later Daniel-authored entry with a
/// removal cue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovalPair {
    /// The session that may be about a removed feature.
    pub session: SessionId,
    /// Its last message.
    pub last_message: OffsetDateTime,
    /// Its header as the model reads it.
    pub header: String,
    /// The later Daniel-authored entry.
    pub later: LaterRecord,
}

/// The removed-feature pairs of one scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemovalCandidates {
    /// Pairs, newest later entry first, at most the cap.
    pub pairs: Vec<RemovalPair>,
    /// Pairs the cap left out.
    pub over_cap: usize,
}

/// A session's header as one text: title, summary, goals and next.
fn header_text(record: &SessionRecord) -> String {
    [
        record.title.as_str(),
        record.summary.as_str(),
        record.goals.as_str(),
        record.next.as_str(),
    ]
    .iter()
    .map(|s| s.trim())
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

/// Whether `text` holds a removal cue as a whole word or phrase.
#[must_use]
pub fn has_removal_cue(text: &str) -> bool {
    let lower = text.to_lowercase();
    REMOVAL_CUES.iter().any(|cue| {
        lower.match_indices(cue).any(|(at, _)| {
            let before = lower.get(..at).and_then(|s| s.chars().next_back());
            let after = lower
                .get(at.saturating_add(cue.len())..)
                .and_then(|s| s.chars().next());
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
    })
}

/// The active Daniel-authored entries written or updated since `since`:
/// the only removal evidence. Session headers and segments are
/// model-written (a pass summarizes a transcript that may hold injected
/// text) and other entries are agent-written (`entry add`), so counting
/// them would let supplied words satisfy the verbatim quote check in
/// [`judge_removals`] and hide a session. Sealed segments stay excluded:
/// sealing proves a segment never changed after it was written, not that
/// Daniel wrote it.
fn later_records(entries: &[Entry], since: OffsetDateTime) -> Vec<LaterRecord> {
    let topics = |t: &crate::record::Topics| t.as_slice().iter().map(ToString::to_string).collect();
    entries
        .iter()
        .filter(|e| {
            e.status == EntryStatus::Active
                && e.updated >= since
                && e.author.kind == AuthorKind::Daniel
        })
        .map(|e| LaterRecord {
            cite: e.id.to_string(),
            record: e.id.to_string(),
            date: e.created,
            text: e.content.clone(),
            topics: topics(&e.topics),
        })
        .collect()
}

/// The scan's removed-feature candidates (§7, S10), model-free: pairs of an
/// active, unpinned session S and a later Daniel-authored entry R whose
/// date is after S's last message, written or updated in the last
/// [`REMOVAL_RECENT_DAYS`] days, that shares a topic or a term with S or
/// overlaps S's header by at least one token in four, and whose text has
/// a removal cue. Model-written session text and agent-written entries
/// never pair. One unit of each record per session, the newest; newest
/// pairs first, at most `max_pairs`.
#[must_use]
pub fn removal_candidates(
    sessions: &[SessionRecord],
    entries: &[Entry],
    now: OffsetDateTime,
    max_pairs: usize,
) -> RemovalCandidates {
    let since = now - time::Duration::days(REMOVAL_RECENT_DAYS);
    let later: Vec<(LaterRecord, BTreeSet<String>)> = later_records(entries, since)
        .into_iter()
        .filter(|l| has_removal_cue(&l.text))
        .map(|l| {
            let tokens = crate::ask::tokens(&l.text).into_iter().collect();
            (l, tokens)
        })
        .collect();
    let mut pairs = Vec::new();
    for s in sessions.iter().filter(|s| s.active && !s.keep_active) {
        let last = activity::last_message(s);
        let header = header_text(s);
        let s_tokens: BTreeSet<String> = crate::ask::tokens(&header).into_iter().collect();
        let s_topics: Vec<String> = s
            .topics
            .as_slice()
            .iter()
            .map(ToString::to_string)
            .collect();
        let s_terms: Vec<String> = s
            .terms
            .iter()
            .map(|t| t.term.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        let mut best: Vec<&LaterRecord> = Vec::new();
        for (l, l_tokens) in &later {
            if l.record == s.id.as_str() || l.date <= last {
                continue;
            }
            let lower = l.text.to_lowercase();
            let related = l.topics.iter().any(|t| s_topics.contains(t))
                || s_terms.iter().any(|t| lower.contains(t.as_str()))
                || overlaps(&s_tokens, l_tokens);
            if !related {
                continue;
            }
            match best.iter_mut().find(|b| b.record == l.record) {
                Some(b) if (l.date, &l.cite) > (b.date, &b.cite) => *b = l,
                Some(_) => {}
                None => best.push(l),
            }
        }
        pairs.extend(best.into_iter().map(|l| RemovalPair {
            session: s.id.clone(),
            last_message: last,
            header: header.clone(),
            later: l.clone(),
        }));
    }
    pairs.sort_by(|a, b| {
        (b.later.date, &b.later.cite)
            .cmp(&(a.later.date, &a.later.cite))
            .then_with(|| a.session.cmp(&b.session))
    });
    let over_cap = pairs.len().saturating_sub(max_pairs);
    pairs.truncate(max_pairs);
    RemovalCandidates { pairs, over_cap }
}

/// The model request for one batch of removed-feature pairs, numbered
/// from 1.
#[must_use]
pub fn removal_request(model: &str, pairs: &[RemovalPair]) -> RemovalRequest {
    RemovalRequest {
        model: model.to_owned(),
        budget: CONSISTENCY_BUDGET,
        pairs: pairs
            .iter()
            .zip(1u32..)
            .map(|(p, n)| RemovalPairItem {
                pair: n.to_string(),
                session: RemovalSession {
                    id: p.session.to_string(),
                    date: p.last_message.date().to_string(),
                    text: p.header.clone(),
                },
                later: RemovalLater {
                    cite: p.later.cite.clone(),
                    date: p.later.date.date().to_string(),
                    text: p.later.text.clone(),
                },
            })
            .collect(),
    }
}

/// One session a `removed` label deactivates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removal {
    /// The session.
    pub session: SessionId,
    /// The model's reason, at most [`activity::REASON_CHARS`] characters.
    pub reason: String,
    /// The later entry's cite.
    pub cite: String,
    /// The verbatim span of the later entry.
    pub quote: String,
}

impl Removal {
    /// The change it makes, by the consistency routine.
    #[must_use]
    pub fn change(&self) -> SetActive {
        SetActive::Deactivate {
            by: Author::consistency(),
            route: ActiveRoute::Removed,
            reason: self.reason.clone(),
            cite: Some(self.cite.clone()),
        }
    }
}

/// What [`judge_removals`] made of one answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemovalJudged {
    /// One per surviving `removed` label, in pair order.
    pub removals: Vec<Removal>,
    /// Pairs labelled `still_current` or `unrelated`.
    pub kept: usize,
    /// Labels dropped: a quote not verbatim in the later entry, an
    /// unknown pair or label, or a second label for the same pair.
    pub dropped: usize,
}

/// Check the model's labels for `pairs`: a `removed` label survives only
/// when the later record cites a Daniel-authored entry (its cite parses
/// as an entry id; [`removal_candidates`] only ever pairs those) and its
/// quote is verbatim in that entry's text. The cite check is defence in
/// depth, so a pair built from model-written or agent-written text
/// deactivates nothing even if it reaches here. The reason is trimmed
/// and cut; an empty one becomes scribe's own sentence naming the
/// feature.
#[must_use]
pub fn judge_removals(answer: RemovalAnswer, pairs: &[RemovalPair]) -> RemovalJudged {
    let mut out = RemovalJudged::default();
    let mut seen = BTreeSet::new();
    let mut found: Vec<(usize, Removal)> = Vec::new();
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
            "still_current" | "unrelated" => out.kept = out.kept.saturating_add(1),
            "removed" => {
                let quote = j.quote.trim();
                if pair.later.cite.parse::<EntryId>().is_err()
                    || !verbatim::contains(&pair.later.text, quote)
                {
                    out.dropped = out.dropped.saturating_add(1);
                    continue;
                }
                let reason = j.reason.trim();
                let reason = if reason.is_empty() {
                    format!(
                        "{} says {} was removed or replaced",
                        pair.later.cite,
                        j.feature.trim()
                    )
                } else {
                    reason.to_owned()
                };
                found.push((
                    index,
                    Removal {
                        session: pair.session.clone(),
                        reason: cut(&reason, activity::REASON_CHARS),
                        cite: pair.later.cite.clone(),
                        quote: quote.to_owned(),
                    },
                ));
            }
            _ => out.dropped = out.dropped.saturating_add(1),
        }
    }
    found.sort_by_key(|(i, _)| *i);
    out.removals = found.into_iter().map(|(_, r)| r).collect();
    out
}

/// The single entry effect used by human approval, agent approval and recovery.
pub(crate) fn apply_approval(proposal: &Proposal, entry: &mut Entry, now: OffsetDateTime) {
    let (status, by) = approved_status(proposal);
    entry.status = status;
    entry.superseded_by = by;
    entry.updated = now;
    if proposal
        .settled_author
        .as_ref()
        .is_some_and(|a| a.kind == crate::AuthorKind::Agent)
    {
        entry.updated_by.clone_from(&proposal.settled_author);
    }
}
