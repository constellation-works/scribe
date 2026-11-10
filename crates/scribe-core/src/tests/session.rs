use super::{T0, session};
use crate::session::{Cadence, SessionControl, SessionFilter, SessionId};
use std::num::{NonZeroU32, NonZeroUsize};
use time::Duration;

#[test]
fn session_ids_are_safe_file_names() {
    assert!(
        "0f8e7a52-3c1d-4b6e-9a2f-5d4c3b2a1e01"
            .parse::<SessionId>()
            .is_ok()
    );
    for bad in ["", ".hidden", "a/b", "..", "a b", &"x".repeat(129)] {
        assert!(bad.parse::<SessionId>().is_err(), "{bad:?}");
    }
}

fn on(due: bool) -> SessionControl {
    SessionControl {
        on: true,
        since: Some(T0),
        due,
        final_pass: false,
        consecutive_failures: 0,
        retry_after: None,
    }
}

fn cadence() -> Cadence {
    Cadence {
        every_minutes: NonZeroU32::new(20).unwrap(),
        every_turns: NonZeroU32::new(10).unwrap(),
    }
}

#[test]
fn a_pass_is_due_after_the_minutes_or_the_turns_and_never_when_off() {
    let c = cadence();
    let last = T0;
    assert!(!c.pass_due(&on(false), last, last + Duration::minutes(19), 9));
    assert!(c.pass_due(&on(false), last, last + Duration::minutes(20), 0));
    assert!(c.pass_due(&on(false), last, last + Duration::minutes(1), 10));
    assert!(c.pass_due(&on(true), last, last, 0), "a due pass stays due");
    let off = SessionControl {
        on: false,
        ..on(true)
    };
    assert!(!c.pass_due(&off, last, last + Duration::hours(5), 99));
}

#[test]
fn the_default_cadence_is_twenty_minutes_or_ten_turns() {
    assert_eq!(Cadence::default(), cadence());
}

#[test]
fn session_selection_filters_then_sorts_by_start() {
    let mut late = session("b", T0 + Duration::hours(1));
    late.topics = vec!["ci".parse().unwrap()].try_into().unwrap();
    let mut other = session("c", T0);
    other.project = "orbit".parse().unwrap();
    let records = vec![late, session("a", T0), other];
    let ids = |f: &SessionFilter| -> Vec<String> {
        f.select(records.clone())
            .items
            .iter()
            .map(|r| r.id.to_string())
            .collect()
    };
    assert_eq!(ids(&SessionFilter::default()), ["a", "c", "b"]);
    let scribe = SessionFilter {
        project: Some("scribe".parse().unwrap()),
        ..SessionFilter::default()
    };
    assert_eq!(ids(&scribe), ["a", "b"]);
    let ci = SessionFilter {
        topic: Some("ci".parse().unwrap()),
        limit: NonZeroUsize::new(5),
        ..SessionFilter::default()
    };
    assert_eq!(ids(&ci), ["b"]);
}
