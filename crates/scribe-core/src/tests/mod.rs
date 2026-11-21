//! Unit tests for the business-logic modules, one file per source file
//! (STD-02 R19). Being siblings rather than children of the modules they test,
//! they reach only `pub(crate)` items. The store's tests live in
//! `store/tests/`.

mod action;
mod activity;
mod ask;
mod capture;
mod config;
mod consistency;
mod control;
mod entry;
mod instruction;
mod pass;
mod proposal;
mod record;
mod search;
mod session;
mod tag;
mod transcript;
mod trigger;
mod verbatim;

use crate::entry::{Cite, ContentFrom, Entry, EntryStatus};
use crate::session::{Segment, SegmentOrigin, SegmentState, SessionRecord, SessionStatus};
use time::OffsetDateTime;
use time::macros::datetime;

/// An active entry `id` created at `created`, in project `scribe`.
pub(crate) fn entry(id: &str, created: OffsetDateTime) -> Entry {
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
        author: crate::record::Author::daniel(),
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
        content: String::from("something"),
    }
}

/// An open session record `id` started at `started`, in project `scribe`.
pub(crate) fn session(id: &str, started: OffsetDateTime) -> SessionRecord {
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
pub(crate) fn segment(n: u32, state: SegmentState, at: OffsetDateTime, text: &str) -> Segment {
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

pub(crate) const T0: OffsetDateTime = datetime!(2026-11-01 10:00:00 UTC);
