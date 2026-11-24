//! `scribe memory` writes (ORB-27707 step 3d): `ack`, `forget`, `restore`
//! and `refresh`, each an audited write (2_design.md §15.4–§15.6).

use crate::cases::memory::MemoryVerbs;
use crate::cases::{Ctx, PortError, resolve};
use crate::memory::NowItem;
use crate::models::ModelError;
use crate::{AuthorName, MemoryShown, Models, Provider, RefreshWritten, SessionId};
use std::time::Duration;

/// `memory refresh`'s model budget, fixed (2_design.md §15.6).
pub const REFRESH_BUDGET: Duration = Duration::from_secs(120);

/// Why a `scribe memory` write did not run.
#[derive(Debug)]
pub enum MemoryError<E> {
    /// A store error or refusal.
    Core(crate::Error),
    /// A port failed.
    Port(E),
    /// The model call failed.
    Model(ModelError),
    /// `forget` without `--confirm`: the item it would forget.
    ConfirmRequired(Box<MemoryShown>),
}

impl<E> From<crate::Error> for MemoryError<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

impl<E> From<ModelError> for MemoryError<E> {
    fn from(err: ModelError) -> Self {
        Self::Model(err)
    }
}

/// `memory ack`: mark the `now` item `id` raised in `session` (an id or
/// unique prefix, resolved across the store and `provider` transcripts) by
/// `author`. The item, and whether this changed it.
///
/// # Errors
///
/// [`MemoryError::Core`]: a refusal or a store error.
pub fn ack<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &str,
    session: &SessionId,
    author: AuthorName,
    provider: Provider,
) -> Result<(NowItem, bool), MemoryError<E>> {
    let session = resolve::session_or_typed(ctx, provider, session)?;
    Ok(ctx.store.ack_memory(id, session, author, ctx.clock.now())?)
}

/// `memory forget`: forget the item `id` for good, only when `confirm`.
///
/// # Errors
///
/// [`MemoryError::ConfirmRequired`] without `confirm` (an `id` that does
/// not exist refuses first), else a refusal or a store error.
pub fn forget<E>(
    ctx: &Ctx<'_, E>,
    id: &str,
    author: AuthorName,
    confirm: bool,
) -> Result<MemoryShown, MemoryError<E>> {
    if !confirm {
        // Names the file, or refuses one that does not exist.
        let shown = ctx.store.read().memory().file(id)?;
        return Err(MemoryError::ConfirmRequired(Box::new(shown)));
    }
    Ok(ctx.store.forget_memory(id, author, ctx.clock.now())?)
}

/// `memory restore`: write profile revision `revision` back. The profile
/// as it now is, and whether this wrote a revision.
///
/// # Errors
///
/// [`MemoryError::Core`]: a missing revision or a store error.
pub fn restore<E>(ctx: &Ctx<'_, E>, revision: i64) -> Result<(MemoryShown, bool), MemoryError<E>> {
    let written = ctx.store.restore_profile(revision, ctx.clock.now())?;
    let shown = ctx.store.read().memory().file("profile")?;
    Ok((shown, written.is_some()))
}

/// `memory refresh` with `models`: read the inputs under the lock, ask the
/// model without it, then check and write under the lock again (§15.6).
/// `None` when nothing is new.
///
/// # Errors
///
/// [`MemoryError`]: the config, the redactor, the model, or the store.
pub fn refresh<E: PortError, M: Models>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    models: &M,
) -> Result<Option<RefreshWritten>, MemoryError<E>> {
    let config = ctx.store.read().tags().config()?.for_provider(provider);
    let redactor = ctx.redaction.redactor(&config).map_err(MemoryError::Port)?;
    let store = ctx.store;
    let inputs = store.profile_inputs(ctx.clock.now())?;
    if inputs.nothing_new() {
        return Ok(None);
    }
    let request = inputs.request(
        config.models.profile.clone(),
        REFRESH_BUDGET,
        config.voice(crate::voice::ModelUse::Profile),
    );
    let answer = models.profile(&request)?;
    Ok(Some(store.write_refresh(
        &inputs,
        &answer,
        &redactor,
        ctx.clock.now(),
    )?))
}
