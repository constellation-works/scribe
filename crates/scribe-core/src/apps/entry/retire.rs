//! `scribe entry retire` (ORB-27707 step 3d).

use super::instruction::{self as instruction_check, Instruction};
use super::verbs::EntryVerbs;
use super::written::{EntryWritten, written};
use crate::AuthorName;
use crate::apps::tag::redactor;
use crate::instruction::{Act, Target};
use crate::runtime::errors::WriteError;
use crate::runtime::{Ctx, PortError};

/// `entry retire` on the agent's reason, and on the user's instruction when
/// one is given. The instruction's session is noted first; the instruction
/// is checked before the store lock, so a refusal changes nothing. A passing
/// check lets retire accept Daniel's entry (2_design.md §12.1).
///
/// # Errors
///
/// [`WriteError`]: a refusal, the instruction check, the redactor, or a
/// store error.
pub fn retire<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &str,
    reason: &str,
    author: AuthorName,
    instruction: &Instruction<'_>,
) -> Result<EntryWritten, WriteError<E>> {
    instruction_check::note(ctx, instruction);
    crate::check_agent_author(&author)?;
    crate::check_reason(reason)?;
    let target = ctx.store.resolve_entry(id)?;
    let instructed = instruction_check::check(
        ctx,
        instruction,
        Act::Retire,
        &Target::new(target.id.to_string(), id),
    )?;
    let redactor = redactor(ctx)?;
    let (entry, changed) = ctx.store.retire_agent_entry(
        id,
        reason,
        instructed.as_ref(),
        author,
        ctx.clock.now(),
        &redactor,
    )?;
    written(ctx, entry, changed)
}
