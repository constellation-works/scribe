//! `scribe memory refresh` (2_design.md §15.6): the profile rebuilt by a
//! model call, an audited write.

use super::Error;
use super::verbs::MemoryVerbs;
use crate::runtime::{Ctx, PortError};
use crate::{Models, Provider, RefreshWritten};
use std::time::Duration;

/// `memory refresh`'s model budget, fixed (2_design.md §15.6).
pub const REFRESH_BUDGET: Duration = Duration::from_secs(120);

/// `memory refresh` with `models`: read the inputs under the lock, ask the
/// model without it, then check and write under the lock again (§15.6).
/// `None` when nothing is new.
///
/// # Errors
///
/// [`Error`]: the config, the redactor, the model, or the store.
pub fn refresh<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    models: &dyn Models,
) -> Result<Option<RefreshWritten>, Error<E>> {
    let config = ctx.store.read().tags().config()?.for_provider(provider);
    let redactor = ctx.redaction.redactor(&config).map_err(Error::Port)?;
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
