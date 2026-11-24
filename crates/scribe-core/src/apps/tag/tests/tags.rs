use super::{T0, entry};
use crate::tag::{TagTable, summarize};

#[test]
fn a_summary_counts_entries_per_tag_and_keeps_unconfigured_ones() {
    let entries = [
        entry("preferences-1", T0),
        entry("preferences-2", T0),
        entry("rituals-1", T0),
    ];
    let rows = summarize(&TagTable::defaults(), &entries);
    let view: Vec<(&str, usize, bool)> = rows
        .iter()
        .map(|r| (r.name.as_str(), r.entries, r.def.is_some()))
        .collect();
    assert_eq!(
        view,
        [
            ("decision", 0, true),
            ("handoff", 0, true),
            ("open-item", 0, true),
            ("preferences", 2, true),
            ("rituals", 1, false),
            ("standard", 0, true)
        ]
    );
}
