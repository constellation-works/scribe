//! `scribe memory ack` (2_design.md §15.5): an audited write.

use super::Error;
use super::verbs::MemoryVerbs;
use crate::memory::NowItem;
use crate::runtime::{Ctx, PortError, resolve};
use crate::{AuthorName, Provider, SessionId};

/// `memory ack`: mark the `now` item `id` raised in `session` (an id or
/// unique prefix, resolved across the store and `provider` transcripts) by
/// `author`. The item, and whether this changed it.
///
/// # Errors
///
/// [`Error::Core`]: a refusal or a store error.
pub fn ack<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &str,
    session: &SessionId,
    author: AuthorName,
    provider: Provider,
) -> Result<(NowItem, bool), Error<E>> {
    let session = resolve::session_or_typed(ctx, provider, session)?;
    Ok(ctx.store.ack_memory(id, session, author, ctx.clock.now())?)
}
