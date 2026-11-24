//! `scribe proposal settle` (ORB-27707 step 3d): an agent settling a
//! proposal on the user's instruction.

use super::verbs::ProposalVerbs;
use crate::apps::entry::{self, Instruction};
use crate::instruction::{Act, Target};
use crate::runtime::errors::{WriteError, redactor};
use crate::runtime::{Ctx, PortError};
use crate::{AuthorName, Proposal, ProposalChange};

/// What `proposal settle` runs.
pub struct ProposalSettleInput<'i> {
    /// The proposal id or unique prefix.
    pub id: &'i str,
    /// Approve (apply) or reject (close without applying).
    pub approve: bool,
    /// A replacement explanation for the settlement.
    pub reason: Option<&'i str>,
    /// The agent.
    pub author: AuthorName,
    /// `--session` and `--instruction`.
    pub instruction: Instruction<'i>,
}

/// `proposal settle`: approve or reject the proposal `id`, on the user's
/// instruction when one is given. The instruction's session is noted first,
/// then the author, the reason and the proposal are checked; the instruction
/// is checked before the store lock, so a refusal changes nothing. A passing
/// check lets settle accept a candidate (2_design.md §12.1); a proposal about
/// Daniel's entry stays his.
///
/// # Errors
///
/// [`WriteError`]: a refusal, the instruction check, the redactor, or a
/// store error.
pub fn settle<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: ProposalSettleInput<'_>,
) -> Result<(Proposal, bool), WriteError<E>> {
    entry::note_instruction(ctx, &input.instruction);
    crate::check_agent_author(&input.author)?;
    if let Some(reason) = input.reason {
        crate::check_reason(reason)?;
    }
    let target = ctx.store.resolve_proposal(input.id)?;
    let instructed = entry::check_instruction(
        ctx,
        &input.instruction,
        if input.approve {
            Act::Approve
        } else {
            Act::Reject
        },
        &Target::new(target.id.to_string(), input.id),
    )?;
    let redactor = redactor(ctx)?;
    Ok(ctx.store.change_agent_proposal(
        input.id,
        if input.approve {
            ProposalChange::Approve
        } else {
            ProposalChange::Reject
        },
        input.reason,
        instructed.as_ref(),
        input.author,
        ctx.clock.now(),
        &redactor,
    )?)
}
