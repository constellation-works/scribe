//! `scribe tag list`: the config and store tags. Read-only: no lock, no
//! audit, no write.

use crate::runtime::Ctx;
use crate::{Result, TagList};

/// Every tag, with what was skipped and the config's warnings.
///
/// # Errors
///
/// A store error.
pub fn list<E>(ctx: &Ctx<'_, E>) -> Result<TagList> {
    ctx.store.read().tags().list()
}
