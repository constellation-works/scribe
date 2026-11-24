//! Record fixtures for unit tests: this crate's own, and those of
//! `scribe-core`'s store, which turns on the `test-support` feature in its
//! dev-dependencies. Never compiled into a shipped build.
#![allow(clippy::unwrap_used)]

use crate::entry::{Cite, ContentFrom, Entry, EntryStatus};
use crate::session::{Segment, SegmentOrigin, SegmentState, SessionRecord, SessionStatus};
use time::OffsetDateTime;
use time::macros::datetime;

/// An active entry `id` created at `created`, in project `scribe`.
pub fn entry(id: &str, created: OffsetDateTime) -> Entry {
    let id: crate::entry::EntryId = id.parse().unwrap();
    Entry {
        tag: id.tag().clone(),
        id,
        status: EntryStatus::Active,
        project: "scribe".parse().unwrap(),
        topics: crate::record::Topics::default(),
        related: Vec::new(),
        provider: String::from("claude-code").try_into().unwrap(),
        host: String::from("host").try_into().unwrap(),
        author: crate::record::Author::user(),
        updated_by: None,
        created,
        updated: created,
        inactive_since: None,
        said: String::from("#scribe preferences: something"),
        content_from: ContentFrom::Instruction,
        retirement_reason: None,
        cite: Some(Cite {
            session: "s-1".parse().unwrap(),
            message: None,
            at: created,
            prompt_sha256: "0".repeat(64),
            line: 1,
            cwd: String::from("/work"),
        }),
        superseded_by: None,
        review_after: None,
        quote: None,
        window: None,
        evidence: None,
        as_of: None,
        approved_by: None,
        instruction: None,
        removal_instruction: None,
        stated_by: None,
        handoff_mode: None,
        decision: None,
        item: None,
        outcome: None,
        content: String::from("something"),
    }
}

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
