use super::{T0, entry};
use crate::entry::TagName;
use crate::tag::{TagDef, TagError, TagTable, summarize};
use std::num::NonZeroU32;
use time::macros::date;

fn name(s: &str) -> TagName {
    s.parse().unwrap()
}

#[test]
fn the_defaults_are_decisions_and_preferences() {
    let table = TagTable::defaults();
    let names: Vec<&str> = table.tags().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "decisions",
            "finding",
            "idea",
            "preferences",
            "remind",
            "standard"
        ]
    );
    assert!(table.action(&name("decisions")).is_none());
    let standard = table.action(&name("standard")).unwrap();
    assert!(standard.require_quote && !standard.ask_first);
    assert!(!table.action(&name("remind")).unwrap().require_quote);
    assert!(table.action(&name("task")).is_none());
    assert!(table.action(&name("friction")).is_none());
    assert_eq!(
        table.resolve("Decision").unwrap().name.as_str(),
        "decisions"
    );
    assert_eq!(table.resolve("pref").unwrap().name.as_str(), "preferences");
    assert_eq!(
        table.resolve(" PREFERENCES ").unwrap().name.as_str(),
        "preferences"
    );
    assert!(table.resolve("rituals").is_none());
}

#[test]
fn a_word_claimed_by_two_tags_is_refused() {
    let tag = |n: &str, aliases: &[&str]| TagDef {
        name: name(n),
        aliases: aliases.iter().map(|a| name(a)).collect(),
        description: None,
        stale_after_days: None,
        action: None,
        require_evidence: false,
    };
    let err =
        TagTable::new(vec![tag("decisions", &["dec"]), tag("declines", &["dec"])]).unwrap_err();
    assert!(
        matches!(err, TagError::Ambiguous { ref word, .. } if word == "dec"),
        "{err:?}"
    );
    let err = TagTable::new(vec![tag("a", &["b"]), tag("b", &[])]).unwrap_err();
    assert!(
        matches!(err, TagError::Ambiguous { .. }),
        "a name and an alias collide"
    );
}

#[test]
fn review_after_counts_the_stale_days_from_creation() {
    let def = TagDef {
        name: name("preferences"),
        aliases: Vec::new(),
        description: None,
        stale_after_days: NonZeroU32::new(180),
        action: None,
        require_evidence: false,
    };
    assert_eq!(
        def.review_after(date!(2026 - 11 - 02)),
        Some(date!(2027 - 05 - 01))
    );
    let never = TagDef {
        stale_after_days: None,
        ..def
    };
    assert_eq!(never.review_after(date!(2026 - 11 - 02)), None);
}

#[test]
fn a_summary_counts_entries_per_tag_and_keeps_unconfigured_ones() {
    let entries = [
        entry("decisions-1", T0),
        entry("decisions-2", T0),
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
            ("decisions", 2, true),
            ("finding", 0, true),
            ("idea", 0, true),
            ("preferences", 0, true),
            ("remind", 0, true),
            ("rituals", 1, false),
            ("standard", 0, true)
        ]
    );
}
