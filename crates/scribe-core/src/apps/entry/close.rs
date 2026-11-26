//! `scribe entry close` (ORB-27707 step 3d): an open item settled by its asker.

use super::verbs::EntryVerbs;
use super::written::{EntryWritten, written};
use crate::apps::tag::redactor;
use crate::runtime::Ctx;
use crate::runtime::errors::WriteError;
use crate::{AuthorName, EntryId};

/// `entry close`: settle an open item with a decision or a note.
///
/// # Errors
///
/// [`WriteError`]: a refusal, the redactor, or a store error.
pub fn close<E>(
    ctx: &Ctx<'_, E>,
    id: &str,
    decision: Option<&EntryId>,
    note: Option<&str>,
    author: AuthorName,
) -> Result<EntryWritten, WriteError<E>> {
    crate::check_agent_author(&author)?;
    if let Some(note) = note {
        crate::check_reason(note)?;
    }
    let redactor = redactor(ctx)?;
    let (entry, changed) =
        ctx.store
            .close_open_item(id, decision, note, author, ctx.clock.now(), &redactor)?;
    written(ctx, entry, changed)
}
