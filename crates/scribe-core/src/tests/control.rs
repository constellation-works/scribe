use super::T0;
use crate::control::{self, Change, Since, Switch};
use crate::session::{Cadence, ControlState, SessionControl, SessionId};
use std::num::NonZeroU32;
use time::Duration;

fn id(s: &str) -> SessionId {
    s.parse().unwrap()
}

#[test]
fn only_a_first_line_slash_scribe_is_a_command() {
    assert_eq!(control::parse("/scribe on"), Some(Switch::On));
    assert_eq!(control::parse("  /scribe OFF  \nmore"), Some(Switch::Off));
    assert_eq!(control::parse("/scribe status"), Some(Switch::Status));
    assert_eq!(control::parse("/scribe"), Some(Switch::Status));
    assert_eq!(control::parse("/scribe:scribe on"), Some(Switch::On));
    assert_eq!(
        control::parse("/scribe update now"),
        Some(Switch::Unknown(String::from("update now")))
    );
    for not in [
        "",
        "please /scribe on",
        "hello\n/scribe on",
        "/scribes on",
        "#scribe decisions: on",
    ] {
        assert_eq!(control::parse(not), None, "{not:?}");
    }
}

#[test]
fn on_marks_a_pass_due_and_on_twice_changes_nothing() {
    let mut state = ControlState::default();
    let s = id("s-1");
    let change = control::switch(&mut state, &s, &Switch::On, T0);
    assert_eq!(change, Change::SwitchedOn);
    assert!(change.changed());
    let on = SessionControl {
        on: true,
        since: Some(T0),
        due: true,
        final_pass: false,
        consecutive_failures: 0,
        retry_after: None,
        pending_links: Vec::new(),
        cataloged_by: None,
    };
    assert_eq!(state.get(&s), Some(&on));

    let before = state.clone();
    let again = control::switch(&mut state, &s, &Switch::On, T0 + Duration::hours(1));
    assert_eq!(again, Change::AlreadyOn(on));
    assert!(!again.changed());
    assert_eq!(state, before, "on twice is a no-op: since is not moved");
}

#[test]
fn off_stops_passes_and_never_creates_a_switch() {
    let mut state = ControlState::default();
    let s = id("s-1");
    assert_eq!(
        control::switch(&mut state, &s, &Switch::Off, T0),
        Change::AlreadyOff
    );
    assert_eq!(
        control::switch(&mut state, &s, &Switch::Status, T0),
        Change::Status(None)
    );
    assert!(state.sessions.is_empty(), "a never-on session stays absent");

    control::switch(&mut state, &s, &Switch::On, T0);
    assert!(control::mark_final(&mut state, &s));
    assert_eq!(
        control::switch(&mut state, &s, &Switch::Off, T0),
        Change::SwitchedOff
    );
    let off = state.get(&s).unwrap();
    assert!(!off.on && !off.due && !off.final_pass);
    assert_eq!(off.since, Some(T0), "when it was switched on is kept");
    assert!(control::due(&state).is_empty());

    // On again later restarts the clock and marks a pass due.
    let later = T0 + Duration::days(1);
    control::switch(&mut state, &s, &Switch::On, later);
    assert_eq!(state.get(&s).unwrap().since, Some(later));
    assert_eq!(control::due(&state).len(), 1);
}

fn cadence() -> Cadence {
    Cadence {
        every_minutes: NonZeroU32::new(20).unwrap(),
        every_turns: NonZeroU32::new(3).unwrap(),
    }
}

fn settled_on(state: &mut ControlState, s: &SessionId) {
    control::switch(state, s, &Switch::On, T0);
    assert!(control::settle(state, s, false));
}

#[test]
fn retry_schedule_grows_to_one_hour_saturates_and_success_or_off_resets_it() {
    let s = id("s-1");
    let mut state = ControlState::default();
    control::fail(&mut state, &s, T0);
    assert!(
        state.sessions.is_empty(),
        "a failure cannot switch a session on"
    );
    control::switch(&mut state, &s, &Switch::On, T0);
    for (n, minutes) in [5, 10, 20, 40, 60, 60, 60].into_iter().enumerate() {
        control::fail(&mut state, &s, T0);
        let c = state.get(&s).unwrap();
        assert_eq!(c.consecutive_failures, u32::try_from(n + 1).unwrap());
        assert_eq!(c.retry_after, Some(T0 + Duration::minutes(minutes)));
        assert!(c.due && c.on, "backoff never gives up");
    }
    state.sessions.get_mut(&s).unwrap().consecutive_failures = u32::MAX;
    control::fail(&mut state, &s, T0);
    assert_eq!(state.get(&s).unwrap().consecutive_failures, u32::MAX);
    assert_eq!(
        state.get(&s).unwrap().retry_after,
        Some(T0 + Duration::hours(1))
    );
    control::settle(&mut state, &s, false);
    assert_eq!(state.get(&s).unwrap().consecutive_failures, 0);
    assert_eq!(state.get(&s).unwrap().retry_after, None);
    control::fail(&mut state, &s, T0);
    control::switch(&mut state, &s, &Switch::Off, T0);
    assert_eq!(state.get(&s).unwrap().consecutive_failures, 0);
    assert_eq!(state.get(&s).unwrap().retry_after, None);
    control::fail(&mut state, &s, T0);
    assert!(!state.get(&s).unwrap().due, "an off session stays off");
}

#[test]
fn stop_marks_due_by_minutes_or_turns_only_when_on() {
    let s = id("s-1");
    let mut state = ControlState::default();
    // The last pass at T0; `minutes` later, with `turns` turns since.
    let since = |minutes: i64, turns: u32| {
        (
            Since {
                last_pass: T0,
                turns,
            },
            T0 + Duration::minutes(minutes),
        )
    };

    // Never switched on: nothing, and no switch is created.
    let (since0, now0) = since(60, 10);
    assert!(!control::mark_due(&mut state, &s, &cadence(), since0, now0));
    assert!(state.sessions.is_empty());

    settled_on(&mut state, &s);
    assert!(control::needs_cadence(&state, &s));
    let (quiet, now) = since(19, 2);
    assert!(!control::mark_due(&mut state, &s, &cadence(), quiet, now));
    let (by_turns, now) = since(1, 3);
    assert!(control::mark_due(&mut state, &s, &cadence(), by_turns, now));
    assert!(!control::needs_cadence(&state, &s), "already due");
    assert!(
        !control::mark_due(&mut state, &s, &cadence(), by_turns, now),
        "already due: unchanged"
    );

    control::settle(&mut state, &s, false);
    let (by_minutes, now) = since(20, 0);
    assert!(control::mark_due(
        &mut state,
        &s,
        &cadence(),
        by_minutes,
        now
    ));

    control::switch(&mut state, &s, &Switch::Off, now);
    let (late, now) = since(600, 100);
    assert!(!control::mark_due(&mut state, &s, &cadence(), late, now));
}

#[test]
fn session_end_marks_final_and_a_final_pass_settles_it() {
    let s = id("s-1");
    let mut state = ControlState::default();
    assert!(!control::mark_final(&mut state, &s), "never on");
    settled_on(&mut state, &s);
    assert!(control::mark_final(&mut state, &s));
    assert!(!control::mark_final(&mut state, &s), "already final");
    let c = state.get(&s).unwrap();
    assert!(c.due && c.final_pass);

    // A non-final pass that was running when the session ended leaves the
    // final pass owed.
    assert!(!control::settle(&mut state, &s, false));
    assert!(state.get(&s).unwrap().due);
    assert!(control::settle(&mut state, &s, true));
    let c = state.get(&s).unwrap();
    assert!(!c.due && !c.final_pass && c.on);
}

#[test]
fn chosen_takes_sessions_that_are_on_whether_or_not_due_and_refuses_the_rest() {
    let mut state = ControlState::default();
    let (a, b, off, never) = (id("s-a"), id("s-b"), id("s-off"), id("s-never"));
    settled_on(&mut state, &b);
    control::switch(&mut state, &a, &Switch::On, T0);
    control::switch(&mut state, &off, &Switch::On, T0);
    control::switch(&mut state, &off, &Switch::Off, T0);
    assert!(!state.get(&b).unwrap().due, "b's due was cleared");

    let picked = control::chosen(&state, &[b.clone(), a.clone(), b.clone()]).unwrap();
    let ids: Vec<&SessionId> = picked.iter().map(|(s, _)| s).collect();
    assert_eq!(ids, [&a, &b], "id order, each once, due or not");

    assert_eq!(control::chosen(&state, &[a.clone(), off.clone()]), Err(off));
    assert_eq!(control::chosen(&state, &[never.clone(), a]), Err(never));
}

#[test]
fn link_commands_parse_with_a_target_and_an_optional_type() {
    use crate::control::{Switch, parse};
    use crate::session::{LinkType, SessionLink};
    let link = |kind, on| Switch::Link {
        link: SessionLink {
            kind,
            session: "s-1".parse().unwrap(),
        },
        on,
    };
    assert_eq!(
        parse("/scribe on --from s-1"),
        Some(link(LinkType::SpawnedFrom, true))
    );
    assert_eq!(
        parse("/scribe link s-1"),
        Some(link(LinkType::SpawnedFrom, false))
    );
    assert_eq!(
        parse("/scribe link s-1 --type continues"),
        Some(link(LinkType::Continues, false))
    );
    assert_eq!(
        parse("/scribe:scribe on --type continues --from s-1"),
        Some(link(LinkType::Continues, true))
    );
    for bad in [
        "/scribe link",
        "/scribe link a/b",
        "/scribe link s-1 s-2",
        "/scribe link s-1 --type related_to",
        "/scribe on --from",
        "/scribe on s-1",
    ] {
        assert!(matches!(parse(bad), Some(Switch::Unknown(_))), "{bad}");
    }
}

#[test]
fn a_link_for_a_session_without_a_record_waits_on_its_switch() {
    use crate::control::{Switch, link, switch};
    use crate::session::{ControlState, LinkError, LinkType, SessionId, SessionLink};
    use std::collections::BTreeMap;
    let mut state = ControlState::default();
    let parent: SessionId = "p".parse().unwrap();
    let child: SessionId = "c".parse().unwrap();
    switch(&mut state, &parent, &Switch::On, T0);
    let to_parent = SessionLink {
        kind: LinkType::SpawnedFrom,
        session: parent.clone(),
    };
    let records = BTreeMap::new();
    // Not cataloged and not switching on: refused, nothing changed.
    let before = state.clone();
    assert_eq!(
        link(&mut state, &records, &child, &to_parent, false, T0),
        Err(LinkError::NotCataloged)
    );
    assert_eq!(state, before);
    let done = link(&mut state, &records, &child, &to_parent, true, T0).unwrap();
    assert!(done.added && done.switched_on && !done.write_record);
    assert_eq!(
        state.get(&child).unwrap().pending_links,
        vec![to_parent.clone()]
    );
    // The parent cannot now continue its child: a cycle through a pending link.
    let back = SessionLink {
        kind: LinkType::Continues,
        session: child.clone(),
    };
    assert!(matches!(
        link(&mut state, &records, &parent, &back, false, T0),
        Err(LinkError::Cycle(_))
    ));
    // With a record, the caller writes the link onto it.
    let records: BTreeMap<SessionId, Vec<SessionLink>> =
        [(child.clone(), Vec::new())].into_iter().collect();
    let mut fresh = ControlState::default();
    switch(&mut fresh, &parent, &Switch::On, T0);
    let done = link(&mut fresh, &records, &child, &to_parent, false, T0).unwrap();
    assert!(done.added && !done.switched_on && done.write_record);
    assert!(fresh.get(&child).is_none());
}
