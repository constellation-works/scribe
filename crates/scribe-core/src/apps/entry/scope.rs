//! Reads that narrow the entries: those of a group's member sessions, and
//! the handoffs that name a session.

use crate::{Entry, Result, SessionId, Snapshot};
use std::collections::BTreeSet;

/// Whether an entry belongs to the `members` sessions, when given; every
/// entry does when not.
pub fn of_members(members: Option<&BTreeSet<SessionId>>) -> impl FnMut(&Entry) -> bool + '_ {
    move |entry| members.is_none_or(|members| crate::group::entry_belongs(entry, members))
}

/// Active `handoff` entries referencing `session:<id>`, newest first.
/// Reads only: no lock, no write.
///
/// # Errors
///
/// The store's read errors.
pub fn handoffs_for_session(read: &Snapshot<'_>, id: &SessionId) -> Result<Vec<Entry>> {
    let (entries, _) = read.entries().all()?;
    Ok(crate::handoff::for_session(&entries, id)
        .into_iter()
        .cloned()
        .collect())
}
