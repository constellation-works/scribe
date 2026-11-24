//! `scribe memory forget` (2_design.md §15.5): an audited write.

use super::Error;
use super::verbs::MemoryVerbs;
use crate::runtime::Ctx;
use crate::{AuthorName, MemoryShown};

/// `memory forget`: forget the item `id` for good, only when `confirm`.
///
/// # Errors
///
/// [`Error::ConfirmRequired`] without `confirm` (an `id` that does
/// not exist refuses first), else a refusal or a store error.
pub fn forget<E>(
    ctx: &Ctx<'_, E>,
    id: &str,
    author: AuthorName,
    confirm: bool,
) -> Result<MemoryShown, Error<E>> {
    if !confirm {
        // Names the file, or refuses one that does not exist.
        let shown = ctx.store.read().memory().file(id)?;
        return Err(Error::ConfirmRequired(Box::new(shown)));
    }
    Ok(ctx.store.forget_memory(id, author, ctx.clock.now())?)
}
