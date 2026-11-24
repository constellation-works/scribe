//! `scribe memory restore` (2_design.md §15.6): an audited write.

use super::Error;
use super::verbs::MemoryVerbs;
use crate::MemoryShown;
use crate::runtime::Ctx;

/// `memory restore`: write profile revision `revision` back. The profile
/// as it now is, and whether this wrote a revision.
///
/// # Errors
///
/// [`Error::Core`]: a missing revision or a store error.
pub fn restore<E>(ctx: &Ctx<'_, E>, revision: i64) -> Result<(MemoryShown, bool), Error<E>> {
    let written = ctx.store.restore_profile(revision, ctx.clock.now())?;
    let shown = ctx.store.read().memory().file("profile")?;
    Ok((shown, written.is_some()))
}
