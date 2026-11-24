//! `scribe entry answer` (ORB-27707 step 3d): the user's answer to an open item.

use super::verbs::EntryVerbs;
use super::verify as entry_verify;
use super::written::{EntryWritten, written};
use crate::runtime::errors::{WriteError, redactor};
use crate::runtime::{Ctx, PortError, resolve};
use crate::{AnswerIn, AuthorName, ItemAnswer, Provider, Related};

/// `entry answer`: the item is checked before any transcript is read, and
/// again by the store under the lock (2_design.md §18.4). An answer
/// recorded here is not an approval in the session that must act (§18.8),
/// so a notice says so when the item asks for its answer in another session.
///
/// # Errors
///
/// [`WriteError`]: a refusal, a transcript read, the redactor, or a store
/// error.
pub fn answer<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &str,
    author: &AuthorName,
    quote: &str,
    related: &[Related],
    provider: Provider,
) -> Result<EntryWritten, WriteError<E>> {
    crate::check_agent_author(author)?;
    let target = ctx.store.resolve_entry(id)?;
    crate::check_unsettled_item(&target)?;
    let related = resolve::relateds(ctx, provider, related)?;
    let (session, at) = entry_verify::answer(ctx, provider, &related, quote)?;
    let answer = ItemAnswer {
        words: quote.to_owned(),
        session,
        at,
        recorded_by: author.clone(),
    };
    let redactor = redactor(ctx)?;
    let (entry, changed) = ctx
        .store
        .answer_open_item(id, answer, ctx.clock.now(), &redactor)?;
    // Base the reminder on the choice that remains stored: an older answer
    // may have been ignored by the store.
    let notice = entry
        .item
        .as_ref()
        .and_then(|item| match (&item.answer_in, &item.answer) {
            (AnswerIn::Session(only), Some(answer)) if *only != answer.session => Some(format!(
                "{} asks for its answer in session:{only}, whose auto mode accepts only what is typed there; the user must also type it in that session",
                entry.id
            )),
            _ => None,
        });
    let mut written = written(ctx, entry, changed)?;
    written.notices.extend(notice);
    Ok(written)
}
