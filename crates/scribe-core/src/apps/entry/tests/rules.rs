//! Action entries: the cancel window, pickups, the stuck count, the
//! per-tag counts and the quote check
//! (2_design.md §4.6).

use super::{T0, entry};
use crate::action::{self, CANCEL_WINDOW, Pickup, STUCK_AFTER};
use crate::entry::{Entry, EntryStatus, Speaker};
use crate::record::{Author, AuthorName};
use crate::transcript::Message;
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
fn an_agent_entry_is_inside_its_cancel_window_until_it_ends() {
    let e = agent("remind-1");
    let fresh = action::state(&e, &[], T0 + CANCEL_WINDOW - Duration::seconds(1));
    assert!(!fresh.past_cancel_window);
    assert_eq!(fresh.cancel_window_ends, Some(T0 + CANCEL_WINDOW));
    assert!(action::state(&e, &[], T0 + CANCEL_WINDOW).past_cancel_window);

    let daniels = entry("remind-2", T0);
    let state = action::state(&daniels, &[], T0);
    assert!(state.past_cancel_window, "Daniel's own entry has no window");
    assert_eq!(state.cancel_window_ends, None);
}

#[test]
fn state_reports_its_own_pickups_oldest_first_and_any_rejection() {
    let e = agent("remind-1");
    let other = agent("remind-2");
    let pickups = [
        pickup(&e, true, 30),
        pickup(&other, false, 10),
        pickup(&e, false, 20),
    ];
    let state = action::state(&e, &pickups, T0 + Duration::hours(2));
    assert_eq!(state.age, Duration::hours(2));
    assert!(state.picked_up && state.rejected);
    let at: Vec<_> = state.pickups.iter().map(|p| p.at).collect();
    assert_eq!(at, [T0 + Duration::minutes(20), T0 + Duration::minutes(30)]);
    assert!(!action::state(&other, &[], T0).picked_up);
}

#[test]
fn stuck_counts_active_unpicked_action_entries_past_the_age_per_tag() {
    let tags = crate::tests::tags_with_idea();
    let mut retired = entry("idea-3", T0);
    retired.status = EntryStatus::Retired;
    let picked = entry("idea-4", T0);
    let entries = [
        entry("idea-1", T0),
        entry("idea-2", T0 + Duration::minutes(30)),
        retired,
        picked.clone(),
        entry("standard-1", T0),
        entry("preferences-1", T0),
    ];
    let now = T0 + STUCK_AFTER;
    let counts = action::stuck(&entries, &tags, &[pickup(&picked, false, 1)], now);
    let view: Vec<(&str, usize)> = counts.iter().map(|(t, n)| (t.as_str(), *n)).collect();
    assert_eq!(
        view,
        [("idea", 1), ("standard", 1)],
        "too young, retired, picked up and memory entries are not stuck"
    );
    assert!(action::stuck(&entries, &tags, &[], T0).is_empty());
}

#[test]
fn a_quote_is_found_only_in_daniels_own_typed_words() {
    let message = |role, text: &str| Message {
        uuid: String::from("m"),
        role,
        at: T0,
        blocks: vec![text.to_owned()],
    };
    let messages = [
        message(Speaker::User, "No — every size needs its unit,  always."),
        message(
            Speaker::Assistant,
            "Understood: sizes carry units from now on.",
        ),
        message(
            Speaker::User,
            "<pasted_content>\nsomeone said pasted words here\n</pasted_content>",
        ),
    ];
    assert!(action::quote_found(
        &messages,
        "every size needs its unit, always"
    ));
    assert!(!action::quote_found(
        &messages,
        "sizes carry units from now on"
    ));
    assert!(!action::quote_found(&messages, "pasted words here"));
    assert!(!action::quote_found(
        &messages,
        "every size needs its unit, never"
    ));
}
