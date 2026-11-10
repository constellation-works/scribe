use super::{T0, entry};
use crate::entry::{EntryError, EntryFilter, EntryId, EntryStatus, TagName};
use std::num::NonZeroUsize;
use time::Duration;

#[test]
fn an_entry_id_is_tag_dash_number_and_round_trips() {
    let id: EntryId = "crew-routing-12".parse().unwrap();
    assert_eq!(id.tag().as_str(), "crew-routing");
    assert_eq!(id.number(), 12);
    assert_eq!(id.to_string(), "crew-routing-12");
}

#[test]
fn malformed_entry_ids_are_refused() {
    for bad in [
        "decisions",
        "decisions-",
        "-3",
        "decisions-0",
        "decisions-012",
        "decisions-+3",
        "Decisions-3",
        "decisions-x",
        "decisions--3",
        "",
    ] {
        assert!(
            matches!(bad.parse::<EntryId>(), Err(EntryError::InvalidId(_))),
            "{bad:?} should be refused"
        );
    }
}

#[test]
fn tag_names_are_lowercase_kebab_case() {
    assert!("decisions".parse::<TagName>().is_ok());
    assert!("crew-routing".parse::<TagName>().is_ok());
    for bad in [
        "",
        "Decisions",
        "a b",
        "a--b",
        "-a",
        "a-",
        "a_b",
        &"x".repeat(33),
    ] {
        assert!(bad.parse::<TagName>().is_err(), "{bad:?}");
    }
}

#[test]
fn the_next_id_is_one_past_the_highest_in_the_same_tag() {
    let tag: TagName = "decisions".parse().unwrap();
    let ids: Vec<EntryId> = ["decisions-3", "decisions-11", "preferences-40"]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
    assert_eq!(EntryId::next_in(&tag, &ids).to_string(), "decisions-12");
    assert_eq!(EntryId::next_in(&tag, &[]).to_string(), "decisions-1");
    let other: TagName = "rituals".parse().unwrap();
    assert_eq!(EntryId::next_in(&other, &ids).to_string(), "rituals-1");
}

#[test]
fn statuses_parse_from_their_names_and_only_active_is_visible() {
    for name in EntryStatus::NAMES {
        let status: EntryStatus = name.parse().unwrap();
        assert_eq!(status.as_str(), name);
        assert_eq!(status.is_hidden(), name != "active");
    }
    assert!("deleted".parse::<EntryStatus>().is_err());
}

#[test]
fn a_filter_matches_every_field_it_sets() {
    let mut e = entry("decisions-1", T0);
    e.topics = vec!["ci".parse().unwrap()].try_into().unwrap();
    let all = EntryFilter::default();
    assert!(all.matches(&e));
    let hit = EntryFilter {
        tag: Some("decisions".parse().unwrap()),
        status: Some(EntryStatus::Active),
        project: Some("scribe".parse().unwrap()),
        topics: vec!["ci".parse().unwrap()],
        ..EntryFilter::default()
    };
    assert!(hit.matches(&e));
    for miss in [
        EntryFilter {
            tag: Some("preferences".parse().unwrap()),
            ..EntryFilter::default()
        },
        EntryFilter {
            status: Some(EntryStatus::Retired),
            ..EntryFilter::default()
        },
        EntryFilter {
            project: Some("orbit".parse().unwrap()),
            ..EntryFilter::default()
        },
        EntryFilter {
            topics: vec!["release".parse().unwrap()],
            ..EntryFilter::default()
        },
    ] {
        assert!(!miss.matches(&e), "{miss:?}");
    }
}

#[test]
fn topics_must_all_match_related_any_and_dates_bound_created() {
    let mut e = entry("decisions-1", T0);
    e.topics = vec!["ci".parse().unwrap(), "api".parse().unwrap()]
        .try_into()
        .unwrap();
    e.related = vec![String::from("task:t-1").try_into().unwrap()];
    let topics = |names: &[&str]| EntryFilter {
        topics: names.iter().map(|t| t.parse().unwrap()).collect(),
        ..EntryFilter::default()
    };
    assert!(topics(&["ci", "api"]).matches(&e));
    assert!(!topics(&["ci", "release"]).matches(&e));
    let related = |refs: &[&str]| EntryFilter {
        related: refs
            .iter()
            .map(|r| String::from(*r).try_into().unwrap())
            .collect(),
        ..EntryFilter::default()
    };
    assert!(related(&["task:t-9", "task:t-1"]).matches(&e));
    assert!(!related(&["task:t-9"]).matches(&e));
    let day = T0.date();
    let dates = |since, until| EntryFilter {
        dates: crate::record::DateRange { since, until },
        ..EntryFilter::default()
    };
    assert!(dates(Some(day), Some(day)).matches(&e));
    assert!(!dates(day.next_day(), None).matches(&e));
    assert!(!dates(None, day.previous_day()).matches(&e));
}

#[test]
fn author_and_author_kind_select_the_writer() {
    let daniel = entry("decisions-1", T0);
    let mut agent = entry("decisions-2", T0);
    agent.author = crate::record::Author::agent("opus".parse().unwrap());
    let by_kind = EntryFilter {
        author_kind: Some(crate::record::AuthorKind::Agent),
        ..EntryFilter::default()
    };
    assert!(by_kind.matches(&agent) && !by_kind.matches(&daniel));
    let by_name = EntryFilter {
        author: Some("daniel".parse().unwrap()),
        ..EntryFilter::default()
    };
    assert!(by_name.matches(&daniel) && !by_name.matches(&agent));
}

#[test]
fn selection_is_oldest_first_and_the_limit_applies_after_the_filter() {
    // STD-01 R33, R34.
    let entries = vec![
        entry("decisions-2", T0 + Duration::hours(2)),
        entry("preferences-1", T0 + Duration::hours(1)),
        entry("decisions-1", T0),
    ];
    let filter = EntryFilter {
        tag: Some("decisions".parse().unwrap()),
        limit: NonZeroUsize::new(1),
        ..EntryFilter::default()
    };
    let listing = filter.select(entries.clone());
    assert_eq!(listing.total, 2);
    assert!(listing.truncated());
    assert_eq!(listing.items[0].id.to_string(), "decisions-1");
    let ids: Vec<String> = EntryFilter::default()
        .select(entries)
        .items
        .iter()
        .map(|e| e.id.to_string())
        .collect();
    assert_eq!(ids, ["decisions-1", "preferences-1", "decisions-2"]);
}
