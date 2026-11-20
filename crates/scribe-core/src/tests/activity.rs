//! `activity`: the `manual`, `age` and `removed` routes of a session's
//! `active` flag (2_design.md §5.7, S10).

use super::{T0, segment, session};
use crate::activity::{
    ActiveOutcome, ActiveRoute, SetActive, age_change, last_message, past_age, set_active,
};
use crate::record::Author;
use crate::session::SegmentState;
use std::num::NonZeroU32;
use time::Duration;

fn agent() -> Author {
    Author::agent("opus".parse().unwrap())
}

fn manual_off(reason: &str) -> SetActive {
    SetActive::Deactivate {
        by: agent(),
        route: ActiveRoute::Manual,
        reason: reason.to_owned(),
        cite: Some(String::from("preferences-3")),
    }
}

fn days(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}

#[test]
fn a_manual_deactivation_records_the_change_and_a_repeat_changes_nothing() {
    let mut record = session("s-1", T0);
    let before = record.clone();
    let now = T0 + Duration::days(3);

    let outcome = set_active(&mut record, &manual_off("  the feature was removed  "), now);

    assert_eq!(outcome, ActiveOutcome::Changed { unpinned: false });
    assert!(!record.active);
    let change = record.active_change.clone().unwrap();
    assert_eq!(change.at, now);
    assert_eq!(change.by, agent());
    assert_eq!(change.route, ActiveRoute::Manual);
    assert_eq!(change.reason, "the feature was removed");
    assert_eq!(change.cite.as_deref(), Some("preferences-3"));
    assert_eq!(record.updated, before.updated, "updated never moves");
    assert_eq!(record.log, before.log, "the text never changes");

    let again = record.clone();
    let outcome = set_active(&mut record, &manual_off("again"), now + Duration::days(1));
    assert_eq!(outcome, ActiveOutcome::Unchanged);
    assert_eq!(record, again, "a repeat changes nothing");
}

#[test]
fn deactivating_a_pinned_session_by_hand_unpins_it() {
    let mut record = session("s-1", T0);
    record.keep_active = true;

    let outcome = set_active(&mut record, &manual_off("gone"), T0);

    assert_eq!(outcome, ActiveOutcome::Changed { unpinned: true });
    assert!(!record.active && !record.keep_active);
}

#[test]
fn an_automatic_route_never_touches_a_pinned_session() {
    let mut record = session("s-1", T0);
    record.keep_active = true;
    let before = record.clone();
    for route in [ActiveRoute::Age, ActiveRoute::Removed] {
        let request = SetActive::Deactivate {
            by: Author::consistency(),
            route,
            reason: String::from("old"),
            cite: None,
        };
        assert_eq!(set_active(&mut record, &request, T0), ActiveOutcome::Pinned);
        assert_eq!(record, before);
    }
}

#[test]
fn activate_shows_pins_and_unpins_and_a_repeat_changes_nothing() {
    let mut record = session("s-1", T0);
    record.active = false;
    let activate = |pin| SetActive::Activate {
        by: agent(),
        reason: None,
        pin,
    };

    assert_eq!(
        set_active(&mut record, &activate(Some(true)), T0),
        ActiveOutcome::Changed { unpinned: false }
    );
    assert!(record.active && record.keep_active);
    let change = record.active_change.clone().unwrap();
    assert_eq!(change.route, ActiveRoute::Manual);
    assert_eq!(change.reason, "activated and pinned by hand");
    assert_eq!(change.cite, None);

    assert_eq!(
        set_active(&mut record, &activate(None), T0),
        ActiveOutcome::Unchanged,
        "already active, pin untouched"
    );
    assert_eq!(
        set_active(&mut record, &activate(Some(true)), T0),
        ActiveOutcome::Unchanged
    );

    let reasoned = SetActive::Activate {
        by: agent(),
        reason: Some(String::from("may drift now")),
        pin: Some(false),
    };
    assert!(set_active(&mut record, &reasoned, T0).changed());
    assert!(record.active && !record.keep_active);
    assert_eq!(record.active_change.unwrap().reason, "may drift now");
}

#[test]
fn a_reason_is_cut_to_three_hundred_characters() {
    let mut record = session("s-1", T0);
    set_active(&mut record, &manual_off(&"x".repeat(400)), T0);
    assert_eq!(record.active_change.unwrap().reason.chars().count(), 300);
}

#[test]
fn age_counts_from_the_last_segment_not_updated() {
    let mut record = session("s-1", T0);
    record.log.push(segment(
        2,
        SegmentState::Open,
        T0 + Duration::days(10),
        "later",
    ));
    record.updated = T0 + Duration::days(200);
    let last = T0 + Duration::days(10) + Duration::hours(1);
    assert_eq!(last_message(&record), last);

    assert!(!past_age(&record, days(120), last + Duration::days(120)));
    assert!(past_age(
        &record,
        days(120),
        last + Duration::days(120) + Duration::seconds(1)
    ));

    let SetActive::Deactivate {
        route,
        reason,
        cite,
        by,
    } = age_change(&record, days(120))
    else {
        panic!("age deactivates");
    };
    assert_eq!(route, ActiveRoute::Age);
    assert_eq!(by, Author::consistency());
    assert_eq!(reason, "no messages since 2026-11-11 (limit 120 days)");
    assert_eq!(cite, None);
}

#[test]
fn age_skips_pinned_and_inactive_sessions_and_a_record_without_a_log_uses_its_start() {
    let now = T0 + Duration::days(400);
    let mut pinned = session("s-1", T0);
    pinned.keep_active = true;
    assert!(!past_age(&pinned, days(120), now));
    let mut inactive = session("s-2", T0);
    inactive.active = false;
    assert!(!past_age(&inactive, days(120), now));

    let mut bare = session("s-3", T0);
    bare.log.clear();
    bare.on_since = T0 + Duration::days(300);
    assert_eq!(last_message(&bare), T0 + Duration::days(300));
    assert!(!past_age(&bare, days(120), now));
}
