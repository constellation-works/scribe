use super::{T0, entry};
use crate::entry::TagName;
use crate::tag::{TagDef, TagError, TagTable, summarize};
use std::num::NonZeroU32;
use time::macros::date;

fn name(s: &str) -> TagName {
    s.parse().unwrap()
}

#[test]
fn the_defaults_are_preferences_and_the_shipped_action_tags() {
    let table = TagTable::defaults();
    let names: Vec<&str> = table.tags().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["idea", "preferences", "standard"]);
    assert!(table.action(&name("preferences")).is_none());
    let standard = table.action(&name("standard")).unwrap();
    assert!(standard.require_quote && !standard.ask_first);
    assert!(!table.action(&name("idea")).unwrap().require_quote);
    assert!(table.action(&name("task")).is_none());
    assert!(table.action(&name("friction")).is_none());
    for word in ["decisions", "decision", "dec", "remind", "finding"] {
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
            ("idea", 0, true),
            ("preferences", 2, true),
            ("rituals", 1, false),
            ("standard", 0, true)
        ]
    );
}

#[test]
fn preferences_ships_propose_idea_ships_write_issue_ships_agent_and_standard_ships_none() {
    use crate::tag::{CaptureRoute, CaptureRules};
    for (tag, route) in [
        ("preferences", CaptureRoute::Propose),
        ("idea", CaptureRoute::Write),
    ] {
        let rules = CaptureRules::shipped(tag);
        assert_eq!(rules.route(), Some(route), "{tag}");
        assert_eq!(rules.writes(), route == CaptureRoute::Write, "{tag}");
        assert!(
            rules.pass_triggers().is_some_and(|t| !t.is_empty()),
            "{tag}"
        );
        assert!(rules.criteria.is_some() && rules.not.is_some(), "{tag}");
    }
    for tag in ["standard", "remind", "finding"] {
        assert_eq!(CaptureRules::shipped(tag), CaptureRules::default(), "{tag}");
    }
    // `issue` ships the prompt hook's rules (2_design.md §11.4), which the
    // pass never takes, and no tag in the table.
    let issue = CaptureRules::shipped("issue");
    assert_eq!(issue.route(), Some(CaptureRoute::Agent));
    assert!(issue.pass_triggers().is_none());
    assert!(
        issue
            .triggers
            .as_ref()
            .is_some_and(|t| t.value.contains(&String::from("root cause")))
    );
    let table = TagTable::defaults();
    assert!(table.resolve("issue").is_none(), "no issue tag ships");
    assert_eq!(table.issue_capture(), issue);
}

#[test]
fn the_propose_and_write_routes_give_the_pass_triggers_and_agent_does_not() {
    use crate::tag::{CaptureRoute, CaptureRules, Rule, RuleOrigin};
    let mut rules = CaptureRules::shipped("preferences");
    for (route, read, writes) in [
        (CaptureRoute::Propose, true, false),
        (CaptureRoute::Write, true, true),
        (CaptureRoute::Agent, false, false),
    ] {
        rules.route = Some(Rule {
            value: route,
            origin: RuleOrigin::Config { line: None },
        });
        assert_eq!(rules.pass_triggers().is_some(), read, "{route:?}");
        assert_eq!(rules.writes(), writes, "{route:?}");
    }
}
