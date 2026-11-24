//! `scribe memory show` (2_design.md §15.4). Read-only: no lock, no audit,
//! no write.

use crate::runtime::Ctx;
use crate::{MemoryShown, MemoryView, ProfileRevision, Result};

/// What `memory show` reads.
pub enum Shown {
    /// A profile revision.
    Revision(ProfileRevision),
    /// One item, or the profile.
    Item(MemoryShown),
    /// The memory block.
    Block(MemoryView),
}

/// Profile revision `revision` when given, else the item `id` when given,
/// else the memory block.
///
/// # Errors
///
/// A missing revision or item, or a store error.
pub fn show<E>(ctx: &Ctx<'_, E>, revision: Option<i64>, id: Option<&str>) -> Result<Shown> {
    if let Some(n) = revision {
        return Ok(Shown::Revision(
            ctx.store.read().memory().profile_revision(n)?,
        ));
    }
    if let Some(id) = id {
        return Ok(Shown::Item(ctx.store.read().memory().file(id)?));
    }
    Ok(Shown::Block(ctx.store.read().memory().view()?))
}
