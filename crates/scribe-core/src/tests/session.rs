use super::{T0, session};
use crate::session::{
    Cadence, SessionControl, SessionFilter, SessionId, SessionOrder, SessionStatus,
};
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
        provider: crate::Provider::ClaudeCode,
        on: true,
        since: Some(T0),
        due,
        final_pass: false,
        consecutive_failures: 0,
        retry_after: None,
        pending_links: Vec::new(),
        cataloged_by: None,
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
        provider: crate::Provider::ClaudeCode,
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

mod links {
    use super::super::{T0, session};
    use crate::session::{
        LinkError, LinkType, SessionId, SessionLink, check_link, children, tree_order,
    };
    use std::collections::{BTreeMap, BTreeSet};

    fn id(s: &str) -> SessionId {
        s.parse().unwrap()
    }

    fn link(kind: LinkType, to: &str) -> SessionLink {
        SessionLink {
            kind,
            session: id(to),
        }
    }

    #[test]
    fn a_link_to_an_unknown_session_itself_or_a_cycle_is_refused() {
        let known: BTreeSet<SessionId> = ["a", "b", "c"].into_iter().map(id).collect();
        // c spawned from b, b spawned from a.
        let graph: BTreeMap<SessionId, Vec<SessionLink>> = [
            (id("b"), vec![link(LinkType::SpawnedFrom, "a")]),
            (id("c"), vec![link(LinkType::SpawnedFrom, "b")]),
        ]
        .into_iter()
        .collect();
        let check = |child: &str, l: SessionLink| check_link(&id(child), &l, &known, &graph);
        assert_eq!(
            check("a", link(LinkType::SpawnedFrom, "zz")),
            Err(LinkError::UnknownSession(id("zz")))
        );
        assert_eq!(
            check("a", link(LinkType::Continues, "a")),
            Err(LinkError::SelfLink)
        );
        assert_eq!(
            check("a", link(LinkType::Continues, "c")),
            Err(LinkError::Cycle(id("c"))),
            "c leads back to a through b"
        );
        assert_eq!(
            check("c", link(LinkType::SpawnedFrom, "a")),
            Err(LinkError::Conflict(id("b"))),
            "one parent"
        );
        assert_eq!(
            check("c", link(LinkType::SpawnedFrom, "b")),
            Ok(false),
            "already"
        );
        assert_eq!(check("c", link(LinkType::Continues, "a")), Ok(true));
        for (err, code) in [
            (LinkError::UnknownSession(id("x")), "link_unknown_session"),
            (LinkError::SelfLink, "link_self"),
            (LinkError::Cycle(id("x")), "link_cycle"),
            (LinkError::Conflict(id("x")), "link_conflict"),
            (LinkError::NotCataloged, "link_session_off"),
        ] {
            assert_eq!(err.code(), code);
        }
    }

    #[test]
    fn children_are_derived_from_the_childrens_links_and_ordered_as_a_tree() {
        let parent = session("p", T0);
        let mut late = session("late", T0 + time::Duration::hours(2));
        late.links = vec![link(LinkType::SpawnedFrom, "p")];
        let mut early = session("early", T0 + time::Duration::hours(1));
        early.links = vec![link(LinkType::Continues, "p")];
        let mut grandchild = session("g", T0 + time::Duration::hours(3));
        grandchild.links = vec![link(LinkType::SpawnedFrom, "early")];
        let lone = session("lone", T0 + time::Duration::hours(4));
        let records = vec![
            parent.clone(),
            early.clone(),
            late.clone(),
            grandchild.clone(),
            lone,
        ];
        let derived = children(&records);
        let of_p: Vec<(&str, LinkType)> = derived[&id("p")]
            .iter()
            .map(|c| (c.session.as_str(), c.kind))
            .collect();
        assert_eq!(
            of_p,
            [
                ("early", LinkType::Continues),
                ("late", LinkType::SpawnedFrom)
            ]
        );
        assert_eq!(derived[&id("early")][0].session, id("g"));
        assert!(!derived.contains_key(&id("lone")));

        let order: Vec<(usize, &str)> = tree_order(&records)
            .into_iter()
            .map(|(depth, i)| (depth, records[i].id.as_str()))
            .collect();
        assert_eq!(
            order,
            [(0, "p"), (1, "early"), (2, "g"), (1, "late"), (0, "lone")]
        );
        // A child whose parent is not listed is a root.
        let order = tree_order(&records[2..]);
        assert_eq!(order.len(), 3);
        assert!(order.iter().all(|(depth, _)| *depth == 0));
    }
}
