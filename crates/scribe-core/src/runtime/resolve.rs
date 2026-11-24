//! Resolving ids the user typed across the store and the provider
//! transcripts on disk (ORB-27707 step 3d), and refusing a session id
//! `#scribe forget` erased.

use crate::runtime::{Ctx, PortError};
use crate::store::forgotten_refusal;
use crate::{Provider, Related, RelatedKind, Result, SessionId, Store};

/// Refuse `id` when `#scribe forget` erased it, before any model call or
/// write on its behalf: a forgotten session takes no write (`session_forgotten`).
/// Reads only.
///
/// # Errors
///
/// `session_forgotten` for a forgotten session, or a store read error.
pub fn refuse_forgotten(store: &Store, id: &SessionId) -> Result<()> {
    if store.read().sessions().is_forgotten(id)? {
        return Err(forgotten_refusal(id, "nothing is written to it"));
    }
    Ok(())
}

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

/// Display names for session ids, unique across the stored sessions, the
/// `provider` transcripts on disk and `extra` (the group members and
/// openers the output names). Cataloged sessions use their stable `s-<n>`
/// aliases; others use prefixes. A failed read leaves full ids available.
pub fn short_ids<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    extra: impl IntoIterator<Item = String>,
) -> crate::ShortIds {
    let mut ids = ctx.store.read().sessions().ids().unwrap_or_default();
    ids.extend(transcript_session_ids(ctx, provider));
    ids.extend(extra);
    let mut shorts = crate::ShortIds::new(ids);
    if let Ok(numbers) = ctx.store.read().sessions().numbers() {
        shorts.set_aliases(
            numbers
                .into_iter()
                .map(|(id, n)| (id.to_string(), format!("s-{n}"))),
        );
    }
    shorts
}
