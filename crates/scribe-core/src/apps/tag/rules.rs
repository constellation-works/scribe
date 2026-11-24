//! The tag app's rules. Pure functions over loaded records: no I/O
//! (STD-02 R5).

use crate::entry::{Entry, EntryStatus, TagName};
use crate::record::AuthorKind;
use crate::store::Pickup;
use crate::tag::TagTable;
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
