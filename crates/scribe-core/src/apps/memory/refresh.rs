//! `scribe memory refresh` / `prepare` / `apply` (2_design.md §15.6, §19.3.5):
//! deterministic preparation and verified, journaled commit for the
//! `scribe-memory-refresh` Orbit job and interactive agents.

use super::Error;
use super::verbs::MemoryVerbs;
pub use super::verbs::{
    ProfileInputs, REFRESH_SESSIONS, REFRESH_SUMMARY_CHARS, REFRESH_VERBATIM_CHARS,
    REFRESH_VERBATIM_SESSIONS, RefreshWritten,
};
use crate::apps::tag::ConfigReads;
use crate::memory::Stem;
use crate::models::{MemoryContext, PROFILE_CHARS, PROMPT_MESSAGE_CHARS, bounded};
use crate::runtime::{Ctx, PortError};
use crate::{Provider, WriteRefusal};
use std::time::Duration;
use time::Date;

pub use scribe_tools::memory_refresh::{
    ApplyRefreshRequest, Basis, PreparedProfileInputs, PreparedRefresh, RefreshCursor,
};

/// `memory refresh`'s model budget, fixed (2_design.md §15.6).
pub const REFRESH_BUDGET: Duration = Duration::from_secs(120);

/// Result of `memory apply` / `scribe.memory-refresh-apply`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyRefreshOutcome {
    /// The rebuilt profile, milestones, removals and cursor committed.
    Written(RefreshWritten),
    /// A `profile_*` validation refusal when `allow_retry` is true: nothing was
    /// written and the Orbit job may run one conditional retry with `correction`.
    Retry {
        /// Stable `profile_*` refusal code.
        code: &'static str,
        /// Actionable correction for the retry prompt.
        correction: String,
    },
}

/// Prepare bounded inputs and optimistic [`Basis`] for `scribe_profile_refresh`
/// (2_design.md §19.3.5). Replays any open `memory_journal` first and returns
/// `{nothing_new: true}` when there is nothing to refresh.
///
/// # Errors
///
/// [`Error`]: the config or the store.
pub fn prepare<E: PortError>(ctx: &Ctx<'_, E>) -> Result<PreparedRefresh, Error<E>> {
    let inputs = ctx.store.profile_inputs(ctx.clock.now())?;
    if inputs.nothing_new() {
        return Ok(PreparedRefresh::nothing_new());
    }
    let config = ctx.store.read().tags().config()?;
    let item_ctx = |stem: &Stem, date: Date, title: &str| MemoryContext {
        stem: stem.to_string(),
        date: crate::record::nullable_date::format(date),
        title: title.to_owned(),
    };
    let session_ids = inputs.sessions.iter().map(|s| s.id.clone()).collect();
    let passed_stems = inputs.passed.iter().map(|n| n.stem.clone()).collect();
    let cursor_to = inputs.cursor_to.map(|(at, session)| RefreshCursor {
        at,
        session: session.to_string(),
    });
    let prepared_inputs = PreparedProfileInputs {
        // The same bounds the in-process prompt applied (STD-08 R1).
        profile: bounded(&inputs.profile, PROFILE_CHARS),
        corrections: inputs.corrections,
        hints: inputs
            .hints
            .iter()
            .map(|hint| bounded(hint, PROMPT_MESSAGE_CHARS))
            .collect(),
        sessions: inputs.sessions,
        now: inputs
            .now
            .iter()
            .map(|n| item_ctx(&n.stem, n.date, &n.title))
            .collect(),
        passed: inputs
            .passed
            .iter()
            .map(|n| item_ctx(&n.stem, n.date, &n.title))
            .collect(),
        milestones: inputs
            .milestones
            .iter()
            .map(|m| item_ctx(&m.stem, m.date, &m.title))
            .collect(),
        preferences: inputs.preferences,
        user_noun: config.user.noun().to_owned(),
        instructions: config
            .instructions
            .get(crate::voice::ModelUse::Profile)
            .map(|text| config.user.fill(text)),
    };
    Ok(PreparedRefresh {
        nothing_new: false,
        basis: Some(inputs.basis),
        cursor_to,
        session_ids,
        backlog: inputs.backlog,
        passed_stems,
        inputs: Some(prepared_inputs),
    })
}

/// Validate and commit a rebuilt profile answer against its prepared [`Basis`]
/// (2_design.md §19.3.5).
///
/// # Errors
///
/// [`Error`]: `memory_changed`, a `profile_*` refusal (when `allow_retry` is
/// false), the config, the redactor, or the store.
pub fn apply<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    request: &ApplyRefreshRequest,
) -> Result<ApplyRefreshOutcome, Error<E>> {
    let config = ctx.store.read().tags().config()?.for_provider(provider);
    let redactor = ctx.redaction.redactor(&config).map_err(Error::Port)?;
    match ctx
        .store
        .write_refresh_prepared(request, &redactor, ctx.clock.now())
    {
        Ok(written) => Ok(ApplyRefreshOutcome::Written(written)),
        Err(err) => {
            if request.allow_retry
                && let Some((code, correction)) = retry_correction(&err)
            {
                return Ok(ApplyRefreshOutcome::Retry { code, correction });
            }
            Err(Error::Core(err))
        }
    }
}

/// Deterministic `scribe memory refresh` (2_design.md §19.3.5, §19.4): replays
/// any open `memory_journal` and archives legacy format-24 items; succeeds with
/// nothing new when no model step is needed, and otherwise directs the caller
/// to `orbit job run scribe-memory-refresh` (or `prepare`/`apply`).
///
/// # Errors
///
/// [`Error::NeedsOrbit`] when sessions, hand edits or passed `now` items need
/// a model rebuild, or a store error.
pub fn refresh_deterministic<E: PortError>(ctx: &Ctx<'_, E>) -> Result<(), Error<E>> {
    let prepared = prepare(ctx)?;
    if prepared.nothing_new {
        return Ok(());
    }
    Err(Error::NeedsOrbit {
        sessions: prepared.session_ids.len(),
        corrections: prepared
            .inputs
            .as_ref()
            .is_some_and(|i| i.corrections.is_some()),
        passed: prepared.passed_stems.len(),
    })
}

fn retry_correction(err: &crate::Error) -> Option<(&'static str, String)> {
    let crate::Error::AgentWrite { kind, .. } = err else {
        return None;
    };
    let correction = match kind {
        WriteRefusal::ProfileShape => format!(
            "Your previous answer was refused (profile_shape): the whole profile must have exactly the seven '## ' headings in order ({}) with nothing before the first heading.",
            crate::models::HEADINGS.join(", ")
        ),
        WriteRefusal::ProfilePinned => String::from(
            "Your previous answer was refused (profile_pinned): copy the Pinned section byte for byte from the input profile without adding, removing or editing any line.",
        ),
        WriteRefusal::ProfileTooLong => format!(
            "Your previous answer was refused (profile_too_long): shorten the profile to at most {} characters in all while keeping all seven headings and Pinned intact.",
            crate::models::PROFILE_CHARS
        ),
        WriteRefusal::ProfileUnknownLink => String::from(
            "Your previous answer was refused (profile_unknown_link): remove or fix any <tag>-<n> reference that is not an existing entry id in the supplied preferences list.",
        ),
        WriteRefusal::ProfileSensitive => String::from(
            "Your previous answer was refused (profile_sensitive): remove any credential, financial detail or sensitive-event wording outside Pinned and from every milestone.",
        ),
        WriteRefusal::ProfileMilestone => format!(
            "Your previous answer was refused (profile_milestone): every milestone must have a valid YYYY-MM-DD date within bounds, a non-empty title (<= {} chars), non-empty text (<= {} chars), and any non-null `now` must match a stem in passed_now.",
            crate::memory::TITLE_CHARS,
            crate::memory::TEXT_CHARS,
        ),
        WriteRefusal::GroupNotFound
        | WriteRefusal::GroupMembership
        | WriteRefusal::GroupEvidenceChanged
        | WriteRefusal::SeatOccupied
        | WriteRefusal::UserOnly
        | WriteRefusal::ReservedAuthor
        | WriteRefusal::InvalidText
        | WriteRefusal::InvalidTopics
        | WriteRefusal::TagNotFound
        | WriteRefusal::TagConflict
        | WriteRefusal::TagConfigured
        | WriteRefusal::ProposalInvalid
        | WriteRefusal::ProposalSettled
        | WriteRefusal::ProposalNotAgent
        | WriteRefusal::EntryNotActive
        | WriteRefusal::EntryNotFound
        | WriteRefusal::ProposalNotFound
        | WriteRefusal::IdAmbiguous
        | WriteRefusal::InvalidId
        | WriteRefusal::QuoteRequired
        | WriteRefusal::QuoteNotFound
        | WriteRefusal::NotActionTag
        | WriteRefusal::CancelWindow
        | WriteRefusal::EvidenceRequired
        | WriteRefusal::FindingCap
        | WriteRefusal::InstructionNotFound
        | WriteRefusal::InstructionStale
        | WriteRefusal::InstructionMismatch
        | WriteRefusal::NotInteractive
        | WriteRefusal::AlreadyCovered
        | WriteRefusal::MemoryNotFound
        | WriteRefusal::MemoryNotNow
        | WriteRefusal::MemoryChanged
        | WriteRefusal::RevisionNotFound
        | WriteRefusal::MigrateBlocked
        | WriteRefusal::ExportTargetExists
        | WriteRefusal::SessionForgotten
        | WriteRefusal::HandoffShape
        | WriteRefusal::HandoffTooLong
        | WriteRefusal::HandoffSession
        | WriteRefusal::NotMemoryTag
        | WriteRefusal::NeedsRedaction
        | WriteRefusal::NotAgentNote
        | WriteRefusal::DecidedByRequired
        | WriteRefusal::AskedByRequired
        | WriteRefusal::RecordFields
        | WriteRefusal::InvalidOptions
        | WriteRefusal::InvalidRecommendation
        | WriteRefusal::RecommendationWhy
        | WriteRefusal::RecordRequired
        | WriteRefusal::DecisionTooLong
        | WriteRefusal::DecisionSession
        | WriteRefusal::DecisionQuoteNotAllowed
        | WriteRefusal::RationaleNotAllowed
        | WriteRefusal::RationaleRequired
        | WriteRefusal::InvalidSettles
        | WriteRefusal::InvalidSupersedes
        | WriteRefusal::SupersedeUserDecision
        | WriteRefusal::NotOpenItem
        | WriteRefusal::ItemSettled
        | WriteRefusal::NotAnswered
        | WriteRefusal::InvalidDecision => return None,
    };
    Some((kind.code(), correction))
}
