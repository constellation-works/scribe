//! `scribe proposal add|update|settle` (ORB-27707 step 3d): an agent's
//! proposal writes. Arguments are refused before an unset `[redact]` env
//! name is reported.

use crate::cases::Ctx;
use crate::cases::entry::EntryVerbs;
use crate::cases::proposal::ProposalVerbs;
use crate::cases::write_error::{WriteError, redactor};
use crate::{AuthorName, Proposal, ProposalChange, ProposalKind, WriteRefusal};

/// What `proposal add` runs.
pub struct ProposalAddInput<'i> {
    /// `stale` or `supersedes`.
    pub kind: ProposalKind,
    /// The entry it is about.
    pub older: &'i str,
    /// The entry that replaces it (`supersedes` only).
    pub newer: Option<&'i str>,
    /// Why.
    pub reason: &'i str,
    /// The agent.
    pub author: AuthorName,
}

/// `proposal add`. The proposal, and whether this wrote it.
///
/// # Errors
///
/// [`WriteError`]: a refusal, the redactor, or a store error.
pub fn add<E>(
    ctx: &Ctx<'_, E>,
    input: ProposalAddInput<'_>,
) -> Result<(Proposal, bool), WriteError<E>> {
    crate::check_agent_author(&input.author)?;
    let needs_newer = input.kind == ProposalKind::Supersedes;
    crate::check_reason(input.reason)?;
    if needs_newer != input.newer.is_some() {
        return Err(WriteError::Core(crate::Error::AgentWrite {
            kind: WriteRefusal::ProposalInvalid,
            message: String::from("supersedes requires --newer; stale forbids it"),
        }));
    }
    ctx.store.resolve_entry(input.older)?;
    if let Some(newer) = input.newer {
        ctx.store.resolve_entry(newer)?;
    }
    let redactor = redactor(ctx)?;
    Ok(ctx.store.add_agent_proposal(
        input.kind,
        input.older,
        input.newer,
        input.reason,
        input.author,
        ctx.clock.now(),
        &redactor,
    )?)
}

/// `proposal update`: change the reason of the proposal `id`.
///
/// # Errors
///
/// [`WriteError`]: a refusal, the redactor, or a store error.
pub fn update<E>(
    ctx: &Ctx<'_, E>,
    id: &str,
    reason: &str,
    author: AuthorName,
) -> Result<(Proposal, bool), WriteError<E>> {
    crate::check_agent_author(&author)?;
    crate::check_reason(reason)?;
    ctx.store.resolve_proposal(id)?;
    let redactor = redactor(ctx)?;
    Ok(ctx.store.change_agent_proposal(
        id,
        ProposalChange::Update,
        Some(reason),
        None,
        author,
        ctx.clock.now(),
        &redactor,
    )?)
}
