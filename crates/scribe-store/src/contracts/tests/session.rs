use crate::fixtures::{T0, session};
use crate::session::{SessionFilter, SessionId, SessionOrder, SessionStatus};
use std::num::NonZeroUsize;
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
        topics: vec!["ci".parse().unwrap()],
        limit: NonZeroUsize::new(5),
        ..SessionFilter::default()
    };
    assert_eq!(ids(&ci), ["b"]);
}

#[test]
fn session_selection_orders_by_updated_newest_first_then_id() {
    let at = |id: &str, started: Duration, updated: Duration| {
        let mut r = session(id, T0 + started);
        r.updated = T0 + updated;
        r
    };
    let records = vec![
        at("b", Duration::hours(1), Duration::hours(1)),
        at("d", Duration::hours(3), Duration::hours(5)),
        at("a", Duration::hours(0), Duration::hours(3)),
        at("c", Duration::hours(2), Duration::hours(5)),
    ];
    let ids = |f: &SessionFilter| -> Vec<String> {
        f.select(records.clone())
            .items
            .iter()
            .map(|r| r.id.to_string())
            .collect()
    };
    let updated = SessionFilter {
        order: SessionOrder::Updated,
        ..SessionFilter::default()
    };
    assert_eq!(
        ids(&updated),
        ["c", "d", "a", "b"],
        "newest updated first, ties by id"
    );
    let newest_two = SessionFilter {
        limit: NonZeroUsize::new(2),
        ..updated.clone()
    };
    assert_eq!(
        ids(&newest_two),
        ["c", "d"],
        "the limit keeps the most recently updated"
    );
    assert_eq!(newest_two.select(records.clone()).total, 4);
    assert_eq!(
        ids(&SessionFilter::default()),
        ["a", "b", "c", "d"],
        "the default stays oldest started first"
    );
}

#[test]
fn a_session_matches_dates_its_span_overlaps_and_its_status() {
    let mut r = session("a", T0);
    r.updated = T0 + time::Duration::days(3);
    let day = T0.date();
    let dates = |since, until| SessionFilter {
        dates: crate::record::DateRange { since, until },
        ..SessionFilter::default()
    };
    // Any day from started to updated is in the span.
    assert!(dates(day.next_day(), day.next_day()).matches(&r));
    assert!(dates(None, Some(day)).matches(&r));
    assert!(!dates(Some(day + time::Duration::days(4)), None).matches(&r));
    assert!(!dates(None, day.previous_day()).matches(&r));
    let status = |s| SessionFilter {
        status: Some(s),
        ..SessionFilter::default()
    };
    assert!(status(SessionStatus::Open).matches(&r));
    assert!(!status(SessionStatus::Final).matches(&r));
    r.related = vec![String::from("task:t-1").try_into().unwrap()];
    let related = SessionFilter {
        related: vec![
            String::from("task:t-2").try_into().unwrap(),
            String::from("task:t-1").try_into().unwrap(),
        ],
        ..SessionFilter::default()
    };
    assert!(related.matches(&r));
    let both = SessionFilter {
        topics: vec!["ci".parse().unwrap(), "api".parse().unwrap()],
        ..SessionFilter::default()
    };
    r.topics = vec!["ci".parse().unwrap()].try_into().unwrap();
    assert!(!both.matches(&r));
}

#[test]
fn short_ids_are_the_shortest_unique_prefix_of_at_least_eight_characters() {
    use crate::ShortIds;
    let a = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";
    let b = "0f8e7a52-3c1d-4b6e-9a2f-5d4c3b2a1e01";
    let shorts = ShortIds::new([a, b]);
    assert_eq!(shorts.short(a), "7c2b9d14");
    assert_eq!(shorts.short(b), "0f8e7a52");
    // UUIDv7s a minute apart share their first eight characters.
    let x = "019a9f3e-5b21-7000-8000-000000000001";
    let y = "019a9f3e-6c44-7000-8000-000000000002";
    let z = "019a9f3e-6c45-7000-8000-000000000003";
    let shorts = ShortIds::new([x, y, z, a]);
    assert_eq!(shorts.short(x), "019a9f3e-5");
    assert_eq!(shorts.short(y), "019a9f3e-6c44");
    assert_eq!(shorts.short(z), "019a9f3e-6c45");
    assert_eq!(shorts.short(a), "7c2b9d14");
    // Every short form resolves to its own id and no other.
    for id in [x, y, z, a] {
        let short = shorts.short(id);
        let hits: Vec<&str> = [x, y, z, a]
            .into_iter()
            .filter(|other| other.starts_with(short))
            .collect();
        assert_eq!(hits, [id]);
    }
}

#[test]
fn a_short_id_is_the_whole_id_when_it_is_short_unknown_or_a_prefix_of_another() {
    use crate::ShortIds;
    let shorts = ShortIds::new(["abc12345", "abc12345-x", "s1"]);
    // An exact id wins over a prefix, so the shorter one prints whole.
    assert_eq!(shorts.short("abc12345"), "abc12345");
    assert_eq!(shorts.short("abc12345-x"), "abc12345-");
    assert_eq!(shorts.short("s1"), "s1");
    assert_eq!(
        shorts.short("not-known-to-the-store"),
        "not-known-to-the-store"
    );
    assert_eq!(ShortIds::default().short("7c2b9d14-8e5f"), "7c2b9d14-8e5f");
}
