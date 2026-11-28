//! The per-tag counts `tag list` shows (2_design.md §4.6).

use crate::action::{self, Pickup};
use crate::entry::{Entry, EntryStatus};
use crate::record::{Author, AuthorName};
use scribe_store::fixtures::{T0, entry};
use time::Duration;

fn agent(id: &str) -> Entry {
    let mut e = entry(id, T0);
    e.author = Author::agent(AuthorName::try_from(String::from("opus")).unwrap());
    e.cite = None;
    e
}

fn pickup(entry: &Entry, rejected: bool, at_minutes: i64) -> Pickup {
    Pickup {
        entry: entry.id.clone(),
        author: Author::agent(AuthorName::try_from(String::from("intake")).unwrap()),
        at: T0 + Duration::minutes(at_minutes),
        result: None,
        rejected,
    }
}

#[test]
fn tag_counts_count_retired_and_rejected_agent_entries_per_action_tag() {
    let tags = crate::tests::catchers::tags_with_idea();
    let mut retired = agent("standard-1");
    retired.status = EntryStatus::Retired;
    let mut forgotten = agent("standard-2");
    forgotten.status = EntryStatus::Forgotten;
    let rejected = agent("idea-1");
    let mut daniels = entry("standard-3", T0);
    daniels.status = EntryStatus::Retired;
    let mut memory = agent("preferences-1");
    memory.status = EntryStatus::Retired;
    let entries = [retired, forgotten, rejected.clone(), daniels, memory];
    let counts = action::tag_counts(&entries, &tags, &[pickup(&rejected, true, 10)]);
    let view: Vec<(&str, (usize, usize))> = counts.iter().map(|(t, n)| (t.as_str(), *n)).collect();
    assert_eq!(view, [("idea", (0, 1)), ("standard", (2, 0))]);
}
