//! Action entries: the cancel window, pickups, the stuck count, the
//! per-tag counts, the quote check and `ask`'s exclusion
//! (2_design.md §4.6).

use super::{T0, entry};
use crate::action::{self, CANCEL_WINDOW, Pickup, STUCK_AFTER};
use crate::ask::{self, Corpus, Question, Scope};
use crate::entry::{Entry, EntryStatus, Speaker, TagName};
use crate::record::{Author, AuthorName};
use crate::tag::TagTable;
use crate::transcript::Message;
use time::Duration;

fn name(s: &str) -> TagName {
    s.parse().unwrap()
}

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
    let tags = TagTable::defaults();
    let mut retired = entry("remind-3", T0);
    retired.status = EntryStatus::Retired;
    let picked = entry("idea-1", T0);
    let entries = [
        entry("remind-1", T0),
        entry("remind-2", T0 + Duration::minutes(30)),
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
        [("remind", 1), ("standard", 1)],
        "too young, retired, picked up and memory entries are not stuck"
    );
    assert!(action::stuck(&entries, &tags, &[], T0).is_empty());
}

#[test]
fn tag_counts_count_retired_and_rejected_agent_entries_per_action_tag() {
    let tags = TagTable::defaults();
    let mut retired = agent("remind-1");
    retired.status = EntryStatus::Retired;
    let mut forgotten = agent("remind-2");
    forgotten.status = EntryStatus::Forgotten;
    let rejected = agent("idea-1");
    let mut daniels = entry("remind-3", T0);
    daniels.status = EntryStatus::Retired;
    let mut memory = agent("preferences-1");
    memory.status = EntryStatus::Retired;
    let entries = [retired, forgotten, rejected.clone(), daniels, memory];
    let counts = action::tag_counts(&entries, &tags, &[pickup(&rejected, true, 10)]);
    let view: Vec<(&str, (usize, usize))> = counts.iter().map(|(t, n)| (t.as_str(), *n)).collect();
    assert_eq!(view, [("idea", (0, 1)), ("remind", (2, 0))]);
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

#[test]
fn ask_leaves_action_entries_out_unless_the_scope_names_their_tag() {
    let tags = TagTable::defaults();
    let entries = [entry("preferences-1", T0), entry("remind-1", T0)];
    let corpus = Corpus {
        entries: &entries,
        sessions: &[],
        tags: &tags,
    };
    let mut question = Question {
        text: String::from("what next"),
        scope: Scope::default(),
        include_hidden: false,
    };
    let ids = |kept: Vec<Entry>| {
        kept.into_iter()
            .map(|e| e.id.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(ask::without_actions(&question, corpus)),
        ["preferences-1"]
    );
    question.scope.tags = vec![name("remind")];
    assert_eq!(
        ids(ask::without_actions(&question, corpus)),
        ["preferences-1", "remind-1"]
    );
}
