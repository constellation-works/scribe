use super::{T0, entry};
use crate::entry::TagName;
use crate::tag::{TagDef, TagError, TagTable, summarize};
use std::num::NonZeroU32;
use time::macros::date;

fn name(s: &str) -> TagName {
    s.parse().unwrap()
}

#[test]
fn the_defaults_are_preferences_and_the_shipped_finding_and_action_tags() {
    let table = TagTable::defaults();
    let names: Vec<&str> = table.tags().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        ["finding", "idea", "preferences", "remind", "standard"]
    );
    assert!(table.action(&name("preferences")).is_none());
    let standard = table.action(&name("standard")).unwrap();
    assert!(standard.require_quote && !standard.ask_first);
    assert!(!table.action(&name("remind")).unwrap().require_quote);
    assert!(table.action(&name("task")).is_none());
    assert!(table.action(&name("friction")).is_none());
    for word in ["decisions", "decision", "dec"] {
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
            ("finding", 0, true),
            ("idea", 0, true),
            ("preferences", 2, true),
            ("remind", 0, true),
            ("rituals", 1, false),
            ("standard", 0, true)
        ]
    );
}

#[test]
fn preferences_and_idea_ship_propose_rules_and_standard_ships_none() {
    use crate::tag::{CaptureRoute, CaptureRules};
    for tag in ["preferences", "idea"] {
        let rules = CaptureRules::shipped(tag);
        assert_eq!(rules.route(), Some(CaptureRoute::Propose), "{tag}");
        assert!(
            rules.pass_triggers().is_some_and(|t| !t.is_empty()),
            "{tag}"
        );
        assert!(rules.criteria.is_some() && rules.not.is_some(), "{tag}");
    }
    for tag in ["standard", "remind", "finding", "issue"] {
        assert_eq!(CaptureRules::shipped(tag), CaptureRules::default(), "{tag}");
    }
    let table = TagTable::defaults();
    assert!(table.resolve("issue").is_none(), "no issue tag ships");
}

#[test]
fn only_the_propose_route_gives_the_pass_triggers() {
    use crate::tag::{CaptureRoute, CaptureRules, Rule, RuleOrigin};
    let mut rules = CaptureRules::shipped("preferences");
    for route in [CaptureRoute::Write, CaptureRoute::Agent] {
        rules.route = Some(Rule {
            value: route,
            origin: RuleOrigin::Config { line: None },
        });
        assert!(rules.pass_triggers().is_none(), "{route:?}");
    }
}
