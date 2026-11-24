//! `scribe entry forget` (ORB-27707 step 3d).

use super::instruction::{self as instruction_check, Instruction};
use super::verbs::EntryVerbs;
use super::written::{EntryWritten, written};
use crate::AuthorName;
use crate::instruction::{Act, Target};
use crate::runtime::errors::WriteError;
use crate::runtime::{Ctx, PortError};

/// `entry forget`: runs only with an instruction and `confirm`; both
/// refusals, and every check of the instruction, come before the store lock
/// and change nothing (2_design.md §12.4).
///
/// # Errors
///
/// [`WriteError::Usage`] without an instruction,
/// [`WriteError::ConfirmRequired`] without `confirm`, the instruction
/// check, or a store error.
pub fn forget<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &str,
    author: &AuthorName,
    confirm: bool,
    instruction: &Instruction<'_>,
) -> Result<EntryWritten, WriteError<E>> {
    instruction_check::note(ctx, instruction);
    crate::check_agent_author(author)?;
    let target = ctx.store.resolve_entry(id)?;
    let uninstructed = || {
        WriteError::Usage(String::from(
            "entry forget runs only on the user's instruction: pass --session <id> and --instruction '<their words>'",
        ))
    };
    if instruction.session.is_none() || instruction.quote.is_none() {
        return Err(uninstructed());
    }
    if !confirm {
        return Err(WriteError::ConfirmRequired(target.id.to_string()));
    }
    let Some(instructed) = instruction_check::check(
        ctx,
        instruction,
        Act::Forget,
        &Target::new(target.id.to_string(), id),
    )?
    else {
        return Err(uninstructed());
    };
    let (entry, changed) =
        ctx.store
            .forget_entry(id, &instructed, author.clone(), ctx.clock.now())?;
    written(ctx, entry, changed)
}
