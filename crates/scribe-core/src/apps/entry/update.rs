//! `scribe entry update` (ORB-27707 step 3d).

use super::verbs::EntryVerbs;
use super::written::{EntryWritten, written};
use crate::apps::tag::redactor;
use crate::runtime::errors::WriteError;
use crate::runtime::{Ctx, PortError, resolve};
use crate::{AuthorName, EntryPatch, Provider};

/// `entry update`: `patch`'s references are resolved here, the ones to
/// remove kept as given too.
///
/// # Errors
///
/// [`WriteError`]: a refusal, the redactor, or a store error.
pub fn update<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &str,
    mut patch: EntryPatch,
    author: AuthorName,
    provider: Provider,
) -> Result<EntryWritten, WriteError<E>> {
    crate::check_agent_author(&author)?;
    patch.add_related = resolve::relateds(ctx, provider, &patch.add_related)?;
    patch.remove_related =
        resolve::relateds_preserving_input(ctx, provider, &patch.remove_related)?;
    ctx.store.check_agent_entry_update(id, &patch)?;
    let redactor = redactor(ctx)?;
    let (entry, changed) =
        ctx.store
            .update_agent_entry(id, patch, author, ctx.clock.now(), &redactor)?;
    written(ctx, entry, changed)
}
