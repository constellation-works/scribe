//! Resolving ids the user typed across the store and the provider
//! transcripts on disk (ORB-27707 step 3d).

use crate::cases::{Ctx, PortError};
use crate::{Provider, Related, RelatedKind, SessionId};

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

/// Resolve one `--related` or `--result` `session:<id>` reference across the
/// store and `provider` transcripts on disk, leaving other kinds and unknown
/// ids as typed.
///
/// # Errors
///
/// An ambiguous prefix, or a store read error.
pub fn related<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    related: &Related,
) -> crate::Result<Related> {
    if related.kind() != RelatedKind::Session {
        return Ok(related.clone());
    }
    let extra = transcript_session_ids(ctx, provider);
    ctx.store
        .read()
        .sessions()
        .resolve_related_with(related, &extra)
}

/// Resolve `session:<id>` references in `related` across the store and
/// `provider` transcripts on disk, scanning transcripts at most once.
///
/// # Errors
///
/// An ambiguous prefix, or a store read error.
pub fn relateds<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    related: &[Related],
) -> crate::Result<Vec<Related>> {
    if !related.iter().any(|r| r.kind() == RelatedKind::Session) {
        return Ok(related.to_vec());
    }
    let extra = transcript_session_ids(ctx, provider);
    ctx.store
        .read()
        .sessions()
        .resolve_relateds_with(related, &extra)
}

/// [`relateds`] keeping each reference as given too. Existing entries can
/// contain an unknown session reference literally, so lookup and removal
/// must also compare that original value after a matching session is
/// cataloged.
///
/// # Errors
///
/// As [`relateds`].
pub fn relateds_preserving_input<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    related: &[Related],
) -> crate::Result<Vec<Related>> {
    let resolved = relateds(ctx, provider, related)?;
    let mut preserved = Vec::with_capacity(resolved.len() * 2);
    for (input, canonical) in related.iter().zip(resolved) {
        if !preserved.contains(input) {
            preserved.push(input.clone());
        }
        if !preserved.contains(&canonical) {
            preserved.push(canonical);
        }
    }
    Ok(preserved)
}
