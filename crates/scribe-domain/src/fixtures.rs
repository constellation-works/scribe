//! Record fixtures for unit tests: this crate's own, and those of
//! `scribe-core`'s store, which turns on the `test-support` feature in its
//! dev-dependencies. Never compiled into a shipped build. The entry fixture
//! is the store's (`scribe-store`'s `fixtures`), which re-exports these.
#![allow(clippy::unwrap_used)]

use crate::session::{Segment, SegmentOrigin, SegmentState, SessionRecord, SessionStatus};
use time::OffsetDateTime;
use time::macros::datetime;

/// An open session record `id` started at `started`, in project `scribe`.
pub fn session(id: &str, started: OffsetDateTime) -> SessionRecord {
    SessionRecord {
        id: id.parse().unwrap(),
        status: SessionStatus::Open,
        active: true,
        keep_active: false,
        active_change: None,
        cataloged_by: None,
        rebuilt_by: None,
        project: "scribe".parse().unwrap(),
        topics: crate::record::Topics::default(),
        related: Vec::new(),
        links: Vec::new(),
        terms: Vec::new(),
        provider: String::from("claude-code").try_into().unwrap(),
        host: String::from("host").try_into().unwrap(),
        created: started,
        updated: started,
        title: String::from("a session"),
        cwd: String::from("/work"),
        started,
        on_since: started,
        scribed_through: None,
        passes: 1,
        next_open: 0,
        log: vec![segment(1, SegmentState::Open, started, "outcome")],
        summary: String::from("summary"),
        goals: String::from("goals"),
        next: String::from("next"),
    }
}

/// Segment `n` of a pass, from `at` to an hour later, holding `text`.
pub fn segment(n: u32, state: SegmentState, at: OffsetDateTime, text: &str) -> Segment {
    Segment {
        n: std::num::NonZeroU32::new(n).unwrap(),
        heading: format!("part {n}"),
        first_message: Some(format!("m-{n}-first")),
        last_message: Some(format!("m-{n}-last")),
        from: at,
        to: at + time::Duration::hours(1),
        state,
        windows: std::num::NonZeroU32::MIN,
        origin: SegmentOrigin::Pass,
        topics: crate::record::Topics::default(),
        related: Vec::new(),
        text: text.to_owned(),
    }
}

/// The fixtures' reference instant.
pub const T0: OffsetDateTime = datetime!(2026-11-01 10:00:00 UTC);
