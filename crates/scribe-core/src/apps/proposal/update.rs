//! `scribe proposal update` (ORB-27707 step 3d): an agent's proposal's
//! reason. Arguments are refused before an unset `[redact]` env name is
//! reported.

use super::verbs::ProposalVerbs;
use crate::runtime::Ctx;
use crate::runtime::errors::{WriteError, redactor};
use crate::{AuthorName, Proposal, ProposalChange};

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
