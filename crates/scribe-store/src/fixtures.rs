//! Record fixtures for unit tests, behind `test-support`: the entry
//! fixture, beside its contract, and the domain's session fixtures
//! re-exported until those records move (ORB-27921 step 5). Never compiled
//! into a shipped build.
#![allow(clippy::unwrap_used)]

use crate::entry::{Cite, ContentFrom, Entry, EntryStatus};
pub use scribe_domain::fixtures::{T0, segment, session};
use time::OffsetDateTime;

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
