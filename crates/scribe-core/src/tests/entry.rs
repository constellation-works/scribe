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
        topic: Some("ci".parse().unwrap()),
        limit: None,
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
            topic: Some("release".parse().unwrap()),
            ..EntryFilter::default()
        },
    ] {
        assert!(!miss.matches(&e), "{miss:?}");
    }
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
