//! Resolving ids the user typed across the store and the provider
//! transcripts on disk (ORB-27707 step 3d).

use crate::cases::{Ctx, PortError};
use crate::{Provider, SessionId};

/// The session ids of `provider` transcripts on disk, for resolving
/// prefixes of uncataloged sessions alongside the store's own ids. If
/// discovery is incomplete, callers keep resolving against the store alone.
pub fn transcript_session_ids<E: PortError>(ctx: &Ctx<'_, E>, provider: Provider) -> Vec<String> {
    ctx.transcripts
        .files(provider)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, _)| id.to_string())
        .collect()
}

/// Resolve a session id or unique prefix across the store and `provider`
/// transcripts on disk, returning an unknown id as typed so its own check
/// can report it. Ambiguity and short prefixes are still refused.
///
/// # Errors
///
/// An ambiguous or too-short prefix, or a store read error.
pub fn session_or_typed<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    input: &SessionId,
) -> crate::Result<SessionId> {
    let extra = transcript_session_ids(ctx, provider);
    ctx.store
        .read()
        .sessions()
        .resolve_or_typed_with(input, &extra)
}
