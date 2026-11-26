//! `scribe proposal add` (ORB-27707 step 3d): an agent's proposal.
//! Arguments are refused before an unset `[redact]` env name is reported.

use super::verbs::ProposalVerbs;
use crate::apps::entry::{self};
use crate::apps::tag::redactor;
use crate::runtime::Ctx;
use crate::runtime::errors::WriteError;
use crate::{AuthorName, Proposal, ProposalKind, WriteRefusal};

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
    entry::resolve_entry(ctx.store, input.older)?;
    if let Some(newer) = input.newer {
        entry::resolve_entry(ctx.store, newer)?;
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
