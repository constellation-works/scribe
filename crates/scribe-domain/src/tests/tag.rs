use super::{T0, entry};
use crate::entry::TagName;
use crate::tag::{TagDef, TagError, TagTable, summarize};
use std::num::NonZeroU32;
use time::macros::date;

fn name(s: &str) -> TagName {
    s.parse().unwrap()
}

#[test]
fn the_defaults_include_the_agent_written_records() {
    let table = TagTable::defaults();
    let names: Vec<&str> = table.tags().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "decision",
            "handoff",
            "open-item",
            "preferences",
            "standard"
        ]
    );
    assert!(table.action(&name("preferences")).is_none());
    assert!(table.action(&name("handoff")).is_none());
    assert!(table.resolve("handoff").unwrap().requires_quote());
    let standard = table.action(&name("standard")).unwrap();
    assert!(standard.require_quote && !standard.ask_first);
    assert!(
        table.action(&name("idea")).is_none(),
        "idea is a config example"
    );
    assert!(table.action(&name("task")).is_none());
    assert!(table.action(&name("friction")).is_none());
    for (tag, aliases) in [
        ("decision", ["decisions", "dec"]),
        ("open-item", ["open-items", "item"]),
    ] {
        let def = table.get(&name(tag)).unwrap();
        assert!(def.action.is_none());
        assert_eq!(def.capture, crate::tag::CaptureRules::default());
        for alias in aliases {
            assert_eq!(table.resolve(alias), Some(def));
        }
    }
    for word in ["ask", "remind", "finding"] {
        assert!(table.resolve(word).is_none(), "{word} is no longer shipped");
    }
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
        require_quote: false,
        require_evidence: false,
        capture: crate::tag::CaptureRules::default(),
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
        require_quote: false,
        require_evidence: false,
        capture: crate::tag::CaptureRules::default(),
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

#[test]
fn the_shipped_catchers_are_exactly_preference_and_standard() {
    use crate::tag::{CaptureFrom, CaptureRoute, CaptureRules, DEFAULT_MAX_OPEN};
    let table = TagTable::defaults();
    let names: Vec<&str> = table.tags().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "decision",
            "handoff",
            "open-item",
            "preferences",
            "standard"
        ]
    );
    assert_eq!(
        CaptureRules::shipped("handoff"),
        CaptureRules::default(),
        "handoff has no pass capture rules"
    );
    assert_eq!(
        table.resolve("preference").map(|t| t.name.as_str()),
        Some("preferences")
    );
    for tag in ["preferences", "standard"] {
        let rules = CaptureRules::shipped(tag);
        assert_eq!(rules.from(), Some(CaptureFrom::User), "{tag}");
        assert_eq!(rules.route(), Some(CaptureRoute::Propose), "{tag}");
        assert!(!rules.writes(), "{tag}");
        assert!(
            rules.pass_triggers().is_some_and(|t| !t.is_empty()),
            "{tag}"
        );
        assert!(rules.criteria.is_some() && rules.not.is_some(), "{tag}");
        assert_eq!(rules.max_open(), DEFAULT_MAX_OPEN, "{tag}");
    }
    assert!(
        table
            .resolve("standard")
            .and_then(|t| t.action)
            .is_some_and(|a| a.require_quote),
        "standard keeps its window and quote check"
    );
    // `idea` and `issue` are config examples now (specs/catchers.md).
    for tag in ["idea", "issue", "remind", "finding"] {
        assert_eq!(CaptureRules::shipped(tag), CaptureRules::default(), "{tag}");
        assert!(table.resolve(tag).is_none(), "{tag}");
    }
}

#[test]
fn both_routes_give_the_pass_triggers_and_only_write_collects() {
    use crate::tag::{CaptureRoute, CaptureRules, Rule, RuleOrigin};
    let mut rules = CaptureRules::shipped("preferences");
    for (route, writes) in [(CaptureRoute::Propose, false), (CaptureRoute::Write, true)] {
        rules.route = Some(Rule {
            value: route,
            origin: RuleOrigin::Config { line: None },
        });
        assert!(rules.pass_triggers().is_some(), "{route:?}");
        assert_eq!(rules.writes(), writes, "{route:?}");
    }
}
