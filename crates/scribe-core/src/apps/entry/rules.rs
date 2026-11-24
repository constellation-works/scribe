//! The entry app's rules. Pure functions over loaded records: no I/O
//! (STD-02 R5).
//!
//! - The entry side of the settling verbs (docs/design/scribe/2_design.md
//!   §3, §7): what `forget` changes, and the outcome every settling verb
//!   reports.
//! - Action entries: the work items a consumer outside scribe picks up
//!   (§4.6; 4_decisions.md, "Action tags are marked in config, and three
//!   ship"). An entry under an action tag is an ordinary entry; what scribe
//!   adds around it is the **cancel window** ([`CANCEL_WINDOW`]), the
//!   per-entry [`ActionState`] built from its pickups
//!   ([`Pickup`], kept in `pickups.json`, never in the
//!   entry), the count of **stuck** entries the session-start hook prints
//!   ([`stuck`]), and the **quote check** an agent's entry under a
//!   `require_quote` tag must pass ([`quote_found`]).
//! - The checks of an entry write: free text and reasons (which the
//!   proposal, session and group apps share; the entry app is their lowest
//!   user), an agent entry's tag, quote, evidence and handoff fields, and
//!   decision and open-item records (2_design.md §18).
//! - The instruction check ([`instruction`]): whether a write an agent runs
//!   on Daniel's word is backed by a turn he typed (§12.2, §12.5). The
//!   entry, proposal and group apps use it; entry is the lowest.

pub mod instruction;

use crate::apps::tag::check_agent_author;
use crate::entry::{Entry, EntryId, EntryStatus, TagName, is_decision_or_item};
use crate::proposal::{Proposal, ProposalId, ProposalState};
use crate::record::{AuthorKind, patch};
use crate::store::Pickup;
use crate::store::refusal;
use crate::tag::TagTable;
use crate::transcript::{Message, user_turns};
use crate::verbatim;
use crate::{
    AgentEntry, AnswerIn, AuthorName, Decision, EntryPatch, Error, ItemState, OpenItem, Related,
    RelatedKind, Result, SessionId, WriteRefusal,
};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;
use time::{Duration, OffsetDateTime};

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

// --- action entries ------------------------------------------------------

/// How long after it is written an agent-written action entry stays
/// cancellable: no consumer picks it up before then
/// (4_decisions.md, "Agent entries have a five-minute cancel window").
pub const CANCEL_WINDOW: Duration = Duration::minutes(5);

/// How old an active action entry with no pickup must be before the
/// session-start hook counts it as stuck.
pub const STUCK_AFTER: Duration = Duration::hours(1);

/// Where an action entry stands for an intake, at one instant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionState {
    /// How long ago it was created.
    pub age: Duration,
    /// When its cancel window ends: `created` + [`CANCEL_WINDOW`] for an
    /// agent-written entry; `None` for Daniel's own entries.
    pub cancel_window_ends: Option<OffsetDateTime>,
    /// Whether an intake may take it now: past its cancel window (always,
    /// for Daniel's).
    pub past_cancel_window: bool,
    /// Whether any consumer recorded a pickup.
    pub picked_up: bool,
    /// Whether a consumer recorded that its result was rejected.
    pub rejected: bool,
    /// The pickups recorded, oldest first.
    pub pickups: Vec<Pickup>,
}

/// The state of `entry` at `now`, given every pickup in the store.
#[must_use]
pub fn state(entry: &Entry, pickups: &[Pickup], now: OffsetDateTime) -> ActionState {
    let cancel_window_ends =
        (entry.author.kind == AuthorKind::Agent).then(|| entry.created + CANCEL_WINDOW);
    let mut mine: Vec<Pickup> = pickups
        .iter()
        .filter(|p| p.entry == entry.id)
        .cloned()
        .collect();
    mine.sort_by_key(|p| p.at);
    ActionState {
        age: now - entry.created,
        past_cancel_window: cancel_window_ends.is_none_or(|ends| now >= ends),
        cancel_window_ends,
        picked_up: !mine.is_empty(),
        rejected: mine.iter().any(|p| p.rejected),
        pickups: mine,
    }
}

/// Active action entries older than [`STUCK_AFTER`] at `now` that no
/// consumer has picked up, counted per tag. An idea the session pass
/// captured itself is not counted: nobody asked for it to be taken up
/// (2_design.md §11.6).
#[must_use]
pub fn stuck(
    entries: &[Entry],
    tags: &TagTable,
    pickups: &[Pickup],
    now: OffsetDateTime,
) -> BTreeMap<TagName, usize> {
    let mut counts = BTreeMap::new();
    for entry in entries {
        if entry.status == EntryStatus::Active
            && tags.action(&entry.tag).is_some()
            && !crate::voice::is_pass_captured(entry)
            && now - entry.created >= STUCK_AFTER
            && !pickups.iter().any(|p| p.entry == entry.id)
        {
            *counts.entry(entry.tag.clone()).or_default() += 1;
        }
    }
    counts
}

/// Whether `quote` is a verbatim span ([`verbatim::contains`]) of Daniel's
/// own words in one of the typed turns of `messages`: user turns only,
/// pasted blocks, harness wrappers and `/scribe` lines left out.
#[must_use]
pub fn quote_found(messages: &[Message], quote: &str) -> bool {
    user_turns(messages)
        .iter()
        .any(|turn| verbatim::contains(&turn.own, quote))
}

// --- write checks: text ---

/// Refuse blank free text: an entry's content or a proposal's text.
///
/// # Errors
///
/// [`WriteRefusal::InvalidText`] when `text` is empty or only whitespace.
pub fn check_content(text: &str) -> Result<()> {
    required(text)
}

/// Refuse a blank reason, or one over 300 characters.
///
/// # Errors
///
/// [`WriteRefusal::InvalidText`] for either.
pub fn check_reason(reason: &str) -> Result<()> {
    required_reason(reason)
}

/// The refusal for a tag that is not in the table.
pub fn unknown_tag() -> Error {
    refusal(
        WriteRefusal::TagNotFound,
        "unknown tag; run 'scribe tag list'",
    )
}

/// Refuse empty text.
pub fn required(text: &str) -> Result<()> {
    if text.trim().is_empty() {
        return Err(refusal(WriteRefusal::InvalidText, "text must not be empty"));
    }
    Ok(())
}

/// Refuse an empty reason, or one over 300 characters.
pub fn required_reason(reason: &str) -> Result<()> {
    required(reason)?;
    if reason.chars().count() > 300 {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "reason must contain at most 300 characters",
        ));
    }
    Ok(())
}

/// Refuse an instruction without the message id of the turn that backs
/// it: every record of it names that turn, and a store with a blank one
/// does not read.
pub fn required_message(instructed: &crate::instruction::Instructed) -> Result<()> {
    if instructed.message.trim().is_empty() {
        return Err(refusal(
            WriteRefusal::InstructionNotFound,
            "the instruction's turn has no message id; ask the user again",
        ));
    }
    Ok(())
}

// --- write checks: agent entry fields ---

/// A quote belongs on an action tag or a `require_quote` tag, and any tag
/// that requires one gets one. Whether it is verbatim in the user's turns is
/// the caller's check: it needs the transcript.
pub fn check_quote(def: &crate::TagDef, quote: Option<&str>) -> Result<()> {
    if def.requires_quote() {
        match quote {
            None => Err(refusal(
                WriteRefusal::QuoteRequired,
                if def.name.as_str() == crate::handoff::HANDOFF_TAG {
                    format!(
                        "{} requires the user's request: pass --quote with their words and --related session:<id>",
                        def.name
                    )
                } else {
                    format!(
                        "{} requires the user's words: pass --quote with the user's correction and --related session:<id>; without a quote, file a friction instead",
                        def.name
                    )
                },
            )),
            Some(quote) if quote.trim().is_empty() => Err(refusal(
                WriteRefusal::QuoteRequired,
                "--quote must not be empty",
            )),
            Some(_) => Ok(()),
        }
    } else {
        match (def.action, quote) {
            (None, Some(_)) => Err(refusal(
                WriteRefusal::NotActionTag,
                format!(
                    "{} is not an action tag; --quote applies only to an action tag or a require_quote tag",
                    def.name
                ),
            )),
            (Some(_), Some(quote)) if quote.trim().is_empty() => Err(refusal(
                WriteRefusal::QuoteRequired,
                "--quote must not be empty",
            )),
            _ => Ok(()),
        }
    }
}

/// Refuse `--spoken` under a tag that has its own entry fields.
pub fn check_spoken_tag(def: &crate::TagDef) -> Result<()> {
    if def.name.as_str() == crate::handoff::HANDOFF_TAG || is_decision_or_item(def.name.as_str()) {
        return Err(refusal(
            WriteRefusal::NotMemoryTag,
            format!("{} has its own entry fields; omit --spoken", def.name),
        ));
    }
    if def.action.is_some() {
        return Err(refusal(
            WriteRefusal::NotMemoryTag,
            format!(
                "{} is an action tag; --spoken writes only non-action memory tags (such as preferences)",
                def.name
            ),
        ));
    }
    if def.require_evidence {
        return Err(refusal(
            WriteRefusal::NotMemoryTag,
            format!(
                "{} requires evidence; --spoken writes only non-action memory tags",
                def.name
            ),
        ));
    }
    Ok(())
}

/// Refuse `--spoken` without the user's words.
pub fn check_spoken_quote(quote: Option<&str>) -> Result<()> {
    match quote {
        None => Err(refusal(
            WriteRefusal::QuoteRequired,
            "--spoken requires the user's words: pass --quote '<their words>' and --related session:<id>",
        )),
        Some(q) if q.trim().is_empty() => Err(refusal(
            WriteRefusal::QuoteRequired,
            "--quote must not be empty",
        )),
        Some(_) => Ok(()),
    }
}

/// A handoff entry's body must carry the six fixed headings in order and fit
/// within [`crate::handoff::MAX_CHARS`] (2_design.md §16).
pub fn check_handoff(tag: &TagName, content: &str) -> Result<()> {
    if tag.as_str() != crate::handoff::HANDOFF_TAG {
        return Ok(());
    }
    crate::handoff::check(content).map(|_| ()).map_err(|err| {
        let kind = match err {
            crate::HandoffError::TooLong { .. } => WriteRefusal::HandoffTooLong,
            crate::HandoffError::Preamble
            | crate::HandoffError::MissingHeadings(_)
            | crate::HandoffError::DuplicateHeading(_)
            | crate::HandoffError::UnknownHeading(_)
            | crate::HandoffError::OutOfOrder(_) => WriteRefusal::HandoffShape,
        };
        refusal(kind, err.to_string())
    })
}

/// Refuse a new `handoff` entry whose `related` references do not name
/// exactly one distinct valid `session:<id>` (2_design.md §16.1).
///
/// # Errors
///
/// [`WriteRefusal::QuoteRequired`] when no `session:` reference is present, or
/// [`WriteRefusal::HandoffSession`] when a `session:` target is invalid or
/// more than one distinct session is named.
pub fn check_handoff_related(tag: &TagName, related: &[crate::Related]) -> Result<()> {
    if tag.as_str() != crate::handoff::HANDOFF_TAG {
        return Ok(());
    }
    crate::handoff::check_related(related)
        .map(|_| ())
        .map_err(|err| {
            let kind = match err {
                crate::HandoffSessionError::Missing => WriteRefusal::QuoteRequired,
                crate::HandoffSessionError::Invalid { .. }
                | crate::HandoffSessionError::Multiple { .. } => WriteRefusal::HandoffSession,
            };
            refusal(kind, err.to_string())
        })
}

/// Refuse an `entry update` patch that would add, remove, or replace a
/// `handoff` entry's `session:` reference, which would attach the handoff to
/// an unverified session or orphan it (2_design.md §16.1).
pub fn check_handoff_related_update(
    entry: &Entry,
    add_related: &[crate::Related],
    remove_related: &[crate::Related],
) -> Result<()> {
    if entry.tag.as_str() != crate::handoff::HANDOFF_TAG {
        return Ok(());
    }
    let mut patched = entry.related.clone();
    patch(&mut patched, add_related, remove_related);
    if crate::handoff::session_refs_changed(&entry.related, &patched) {
        return Err(refusal(
            WriteRefusal::HandoffSession,
            "a handoff's session reference cannot be added, removed or replaced; retire this handoff and add a new one for the target session instead",
        ));
    }
    Ok(())
}

/// An evidence pointer belongs on a `require_evidence` (finding) entry,
/// and such a tag gets one: a result with nothing to check it against is
/// just an agent's memory of itself.
pub fn check_evidence(def: &crate::TagDef, evidence: Option<&str>) -> Result<()> {
    match (def.require_evidence, evidence) {
        (true, None) => Err(refusal(
            WriteRefusal::EvidenceRequired,
            format!(
                "{} records a measured or verified result: pass --evidence with a pointer to its evidence (a path, task, run, experiment or commit)",
                def.name
            ),
        )),
        (true, Some(evidence)) if evidence.trim().is_empty() => Err(refusal(
            WriteRefusal::EvidenceRequired,
            "--evidence must not be empty",
        )),
        (false, Some(_)) => Err(refusal(
            WriteRefusal::EvidenceRequired,
            format!(
                "{} does not take an evidence pointer; --evidence applies only to the finding tag",
                def.name
            ),
        )),
        _ => Ok(()),
    }
}

/// The session a direct finding write names (a `session:` reference), as
/// the per-session cap counts it.
pub fn draft_session(related: &[crate::Related]) -> Option<String> {
    related
        .iter()
        .find(|r| r.kind() == crate::RelatedKind::Session)
        .map(|r| r.target().to_owned())
}

/// Whether `entry` counts against this write's per-session finding cap:
/// same named session, or, when the write names none, the same author's
/// unnamed-session findings of the same day.
pub fn counts_toward_cap(
    entry: &Entry,
    tag: &TagName,
    session: Option<&str>,
    author: &AuthorName,
    now: OffsetDateTime,
) -> bool {
    if &entry.tag != tag || !entry.is_finding() {
        return false;
    }
    match (session, entry.session_key()) {
        (Some(named), Some(held)) => named == held,
        (None, None) => &entry.author.name == author && entry.created.date() == now.date(),
        _ => false,
    }
}

// --- write checks: decision and open-item records ---

/// What a trial entry's synthesized `item` says waited, since the trial
/// recorded it only in prose (2_design.md §18.7).
const TRIAL_BLOCKED: &str = "not recorded (trial entry)";

const DECISION_MAX_CHARS: usize = 3_000;

pub(crate) fn check_options(options: &[String], recommended: Option<usize>) -> Result<()> {
    if !(options.is_empty() || (2..=6).contains(&options.len()))
        || options.iter().any(|option| option.trim().is_empty())
    {
        return Err(refusal(
            WriteRefusal::InvalidOptions,
            "supply either zero or two to six nonblank --option values",
        ));
    }
    if recommended.is_some_and(|n| !(1..=options.len()).contains(&n)) {
        return Err(refusal(
            WriteRefusal::InvalidRecommendation,
            "--recommended must be the 1-based index of a supplied option",
        ));
    }
    Ok(())
}

pub(crate) fn check_record_reference(related: &[Related]) -> Result<()> {
    if !related.iter().any(|r| {
        matches!(
            r.kind(),
            RelatedKind::Task | RelatedKind::Pr | RelatedKind::Repo
        )
    }) {
        return Err(refusal(
            WriteRefusal::RecordRequired,
            "a decision needs a record: pass --related task:<id>, pr:<owner>/<repo>#<n> or repo:<slug>",
        ));
    }
    Ok(())
}

/// Refuse a decision too large to store.
pub fn check_decision_size(decision: &Decision, content: &str, quote: Option<&str>) -> Result<()> {
    if decision.chars(content, quote) > DECISION_MAX_CHARS {
        return Err(refusal(
            WriteRefusal::DecisionTooLong,
            "a decision holds at most 3,000 characters across its content, options, rationale and quotes",
        ));
    }
    Ok(())
}

/// Refuse a decision whose fields do not fit who decided it.
pub fn check_decision(decision: &Decision, draft: &AgentEntry) -> Result<()> {
    let quote = draft.quote.as_deref();
    if decision.by_user() {
        if decision.rationale.is_some() {
            return Err(refusal(
                WriteRefusal::RationaleNotAllowed,
                "a user decision takes only the user's own reason in --reason-quote",
            ));
        }
        if !quote.is_some_and(has_text) {
            return Err(refusal(
                WriteRefusal::QuoteRequired,
                "a user decision requires --quote with the user's words",
            ));
        }
        if decision
            .reason_quote
            .as_deref()
            .is_some_and(|reason| !has_text(reason))
        {
            return Err(refusal(
                WriteRefusal::QuoteNotFound,
                "--reason-quote must contain the user's words",
            ));
        }
        if !draft
            .related
            .iter()
            .any(|r| r.kind() == RelatedKind::Session && r.target().parse::<SessionId>().is_ok())
        {
            return Err(refusal(
                WriteRefusal::QuoteRequired,
                "pass --related session:<id> for the user's decision",
            ));
        }
    } else {
        check_agent_author(&decision.decided_by)?;
        if quote.is_some() || decision.reason_quote.is_some() {
            return Err(refusal(
                WriteRefusal::DecisionQuoteNotAllowed,
                "--quote and --reason-quote apply to decisions made by the user",
            ));
        }
        if !decision.rationale.as_deref().is_some_and(has_text) {
            return Err(refusal(
                WriteRefusal::RationaleRequired,
                "an agent decision requires --rationale",
            ));
        }
    }
    check_options(&decision.options, decision.recommended)?;
    check_record_reference(&draft.related)?;
    check_decision_size(decision, &draft.content, quote)
}

pub(crate) fn has_text(value: &str) -> bool {
    !value.trim().is_empty()
}

/// Refuse an open item with an invalid asker, blocker or options.
pub fn check_open_item(item: &OpenItem) -> Result<()> {
    check_agent_author(&item.asked_by)?;
    check_content(&item.blocked)?;
    check_options(&item.options, item.recommended)?;
    if !item.options.is_empty() && item.recommended.is_none() {
        return Err(refusal(
            WriteRefusal::InvalidRecommendation,
            "a choice needs --recommended with the 1-based index of an option",
        ));
    }
    if item.options.is_empty() && item.why.is_some()
        || !item.options.is_empty() && !item.why.as_deref().is_some_and(has_text)
    {
        return Err(refusal(
            WriteRefusal::RecommendationWhy,
            "--why requires options and must explain the recommendation for a choice",
        ));
    }
    if item.answer.is_some() || item.closed.is_some() {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "a new open item starts without an answer or closure",
        ));
    }
    Ok(())
}

/// Refuse a decision whose quoted text redaction would alter.
pub fn check_decision_redaction(draft: &AgentEntry, mask: &dyn Fn(&str) -> String) -> Result<()> {
    if let Some(decision) = &draft.decision
        && draft
            .quote
            .iter()
            .chain(&decision.reason_quote)
            .any(|quote| mask(quote) != *quote)
    {
        return Err(refusal(
            WriteRefusal::NeedsRedaction,
            "the decision has quoted text that redaction would alter",
        ));
    }
    Ok(())
}

/// Refuse an update that changes a decision as it may not change.
pub fn check_decision_update(before: &Entry, after: &Entry) -> Result<()> {
    if let Some(decision) = &after.decision {
        check_record_reference(&after.related)?;
        if decision.by_user()
            && crate::handoff::session_refs_changed(&before.related, &after.related)
        {
            return Err(refusal(
                WriteRefusal::DecisionSession,
                "the source sessions of a user decision are fixed",
            ));
        }
        check_decision_size(decision, &after.content, after.quote.as_deref())?;
    }
    Ok(())
}

/// Apply `patch`'s decision fields to `entry`, masked by `mask`.
pub fn patch_decision(
    entry: &mut Entry,
    patch: &EntryPatch,
    mask: &dyn Fn(&str) -> String,
) -> Result<()> {
    if patch.decision_options.is_none()
        && patch.decision_recommended.is_none()
        && patch.decision_rationale.is_none()
    {
        return Ok(());
    }
    let decision = entry.decision.as_mut().ok_or_else(|| refusal(
        WriteRefusal::RecordFields,
        "--option, --clear-options, --recommended and --rationale on entry update require a structured decision",
    ))?;
    if let Some(rationale) = &patch.decision_rationale {
        if decision.by_user() {
            return Err(refusal(
                WriteRefusal::RationaleNotAllowed,
                "a user decision's reason is its checked --reason-quote",
            ));
        }
        if !has_text(rationale) {
            return Err(refusal(
                WriteRefusal::RationaleRequired,
                "an agent decision requires a nonblank rationale",
            ));
        }
        decision.rationale = Some(mask(rationale));
    }
    if let Some(options) = &patch.decision_options {
        check_options(options, patch.decision_recommended)?;
        decision.options = options.iter().map(|option| mask(option)).collect();
        decision.recommended = patch.decision_recommended;
    } else if let Some(recommended) = patch.decision_recommended {
        check_options(&decision.options, Some(recommended))?;
        decision.recommended = Some(recommended);
    }
    Ok(())
}

/// The derived state of an item `entry answer` or `entry close` may still
/// settle: refuse an entry not under `open-item`, and a closed or withdrawn
/// one, so a caller can refuse before it reads any transcript.
///
/// # Errors
///
/// `not_open_item` and `item_settled`.
pub fn check_unsettled_item(entry: &Entry) -> Result<ItemState> {
    let state = ItemState::of(entry).ok_or_else(|| {
        refusal(
            WriteRefusal::NotOpenItem,
            format!(
                "{} is not an open item; entry answer and entry close take only entries under open-item",
                entry.id
            ),
        )
    })?;
    if state.is_settled() {
        return Err(refusal(
            WriteRefusal::ItemSettled,
            format!("{} is already {state}; nothing written", entry.id),
        ));
    }
    Ok(state)
}

/// The `item` object of an open item, synthesized for a trial entry that
/// has none: an action item asked by its author, with nothing typed about
/// what waits.
pub fn item_mut(entry: &mut Entry) -> &mut OpenItem {
    let asked_by = entry.author.name.clone();
    entry.item.get_or_insert_with(|| OpenItem {
        asked_by,
        options: Vec::new(),
        recommended: None,
        why: None,
        blocked: TRIAL_BLOCKED.to_owned(),
        answer_in: AnswerIn::Any,
        answer: None,
        closed: None,
    })
}

// --- group membership ---

/// Whether an entry belongs to one of these transcripts through its source
/// cite or an explicit session reference (as used by handoffs).
#[must_use]
pub fn entry_belongs(entry: &crate::Entry, members: &BTreeSet<SessionId>) -> bool {
    entry
        .cite
        .as_ref()
        .is_some_and(|cite| members.contains(&cite.session))
        || entry
            .related
            .iter()
            .filter(|related| related.kind() == RelatedKind::Session)
            .filter_map(|related| related.target().parse::<SessionId>().ok())
            .any(|id| members.contains(&id))
}
