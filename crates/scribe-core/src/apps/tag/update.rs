//! `scribe tag update` (ORB-27707 step 3d): an agent tag, without changing
//! config.toml.

use super::verbs::TagVerbs;
use crate::runtime::Ctx;
use crate::runtime::errors::{WriteError, redactor};
use crate::{AuthorName, StoreTag, TagName, TagPatch};

/// Update the agent tag `name`. The tag, and whether this changed it.
///
/// # Errors
///
/// [`WriteError`]: a refusal, the redactor, or a store error.
pub fn update<E>(
    ctx: &Ctx<'_, E>,
    name: &TagName,
    patch: TagPatch,
    author: AuthorName,
) -> Result<(StoreTag, bool), WriteError<E>> {
    crate::check_agent_author(&author)?;
    if let Some(description) = &patch.description {
        crate::check_description(description)?;
    }
    ctx.store.check_agent_tag_update(name, &patch)?;
    let redactor = redactor(ctx)?;
    Ok(ctx
        .store
        .update_agent_tag(name, patch, author, ctx.clock.now(), &redactor)?)
}
