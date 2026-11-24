//! `scribe mark list`: the markers scribe wrote no entry for. Read-only: no
//! lock, no audit, no write.

use crate::annotation::MarkRecord;
use crate::runtime::{Ctx, PortError, resolve};
use crate::{Provider, Result, SessionId};

/// `mark list`: the markers scribe wrote no entry for (pending, given up
/// or handed to the agent), of `session` when
/// given, resolved across the store and the `provider` transcripts on disk
/// (an unknown id is kept as typed). The session as resolved, and the
/// markers.
///
/// # Errors
///
/// An ambiguous or too-short prefix, or a store error.
pub fn list<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    session: Option<&SessionId>,
) -> Result<(Option<SessionId>, Vec<MarkRecord>)> {
    let session = session
        .map(|id| resolve::session_or_typed(ctx, provider, id))
        .transpose()?;
    let marks = ctx.store.read().marks().uncataloged(session.as_ref())?;
    Ok((session, marks))
}
