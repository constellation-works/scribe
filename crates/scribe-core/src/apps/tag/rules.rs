//! The tag app's rules. Pure functions over loaded records: no I/O
//! (STD-02 R5).
//!
//! - The count of retired and rejected agent entries per action tag.
//! - The checks of an agent-channel write that every app's writes share
//!   ([`check_agent_author`]; the tag app is its lowest user), and of a store
//!   tag write (`scribe tag add`, `scribe tag update`).
//! - [`patch`], the add-and-remove edit of a list field that `tag update`
//!   and `entry update` both apply; the tag app is its lowest user.

pub mod tags;

use crate::entry::{Entry, EntryStatus, TagName};
use crate::record::AuthorKind;
use crate::store::Pickup;
use crate::store::refusal;
use crate::tag::TagTable;
use crate::{AuthorName, Result, WriteRefusal};
use std::collections::BTreeMap;

/// Per action tag: `(retired, rejected)`, the agent-written entries under it
/// that were retired or forgotten, and those whose result was rejected.
#[must_use]
pub fn tag_counts(
    entries: &[Entry],
    tags: &TagTable,
    pickups: &[Pickup],
) -> BTreeMap<TagName, (usize, usize)> {
    let mut counts: BTreeMap<TagName, (usize, usize)> = BTreeMap::new();
    for entry in entries {
        if entry.author.kind != AuthorKind::Agent || tags.action(&entry.tag).is_none() {
            continue;
        }
        let count = counts.entry(entry.tag.clone()).or_default();
        if matches!(entry.status, EntryStatus::Retired | EntryStatus::Forgotten) {
            count.0 += 1;
        }
        if pickups.iter().any(|p| p.entry == entry.id && p.rejected) {
            count.1 += 1;
        }
    }
    counts
}

// --- write checks ---

/// Refuse a reserved name on an agent-channel write: `user` (and `daniel`,
/// its name before format 23), `scribe`, `consistency`, `session-pass` and
/// `tag-retired` name the user's and scribe's own channels, so a record
/// written with one could be mistaken for theirs.
///
/// # Errors
///
/// [`WriteRefusal::ReservedAuthor`] when `author` is reserved.
pub fn check_agent_author(author: &AuthorName) -> Result<()> {
    if author.is_reserved() {
        return Err(refusal(
            WriteRefusal::ReservedAuthor,
            format!(
                "'{author}' is reserved for the user's and scribe's own channels; pass your own crew or model name as --author"
            ),
        ));
    }
    Ok(())
}

/// Refuse a tag description that is blank or over 300 characters.
///
/// # Errors
///
/// [`WriteRefusal::InvalidText`] for either.
pub fn check_description(description: &str) -> Result<()> {
    if description.trim().is_empty() || description.chars().count() > 300 {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "description must contain 1-300 characters",
        ));
    }
    Ok(())
}

/// Remove `remove` from `items`, then append each of `add` not already there.
pub fn patch<T: Clone + PartialEq>(items: &mut Vec<T>, add: &[T], remove: &[T]) {
    items.retain(|item| !remove.contains(item));
    for item in add {
        if !items.contains(item) {
            items.push(item.clone());
        }
    }
}
