//! Action entries: the work items a consumer outside scribe picks up
//! (docs/design/scribe/2_design.md §4.6; 4_decisions.md, "Action tags are
//! marked in config, and three ship").
//!
//! An entry under an action tag is an ordinary entry. This module holds
//! what scribe adds around it, all pure values and no I/O (STD-02 R5):
//!
//! - the **cancel window**: an agent-written entry is not to be picked up
//!   until [`CANCEL_WINDOW`] after it was written, so Daniel's
//!   `#scribe retire <id>` cannot lose the race with a routine;
//! - **pickups** ([`Pickup`]): a consumer's note that it took an entry, and
//!   later, that Daniel rejected its result. They live in `pickups.json`,
//!   never in the entry, which no consumer edits;
//! - the per-entry [`ActionState`] the read surface reports, the count of
//!   **stuck** entries the session-start hook prints ([`stuck`]), and the
//!   per-tag counts `tag list` shows ([`tag_counts`]);
//! - the **quote check** an agent's entry under a `require_quote` tag must
//!   pass ([`quote_found`]).

use crate::entry::{Entry, EntryId, EntryStatus, TagName};
use crate::record::{Author, AuthorKind, Related};
use crate::tag::TagTable;
use crate::transcript::{Message, user_turns};
use crate::verbatim;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use time::{Duration, OffsetDateTime};

/// How long after it is written an agent-written action entry stays
/// cancellable: no consumer picks it up before then
/// (4_decisions.md, "Agent entries have a five-minute cancel window").
pub const CANCEL_WINDOW: Duration = Duration::minutes(5);

/// How old an active action entry with no pickup must be before the
/// session-start hook counts it as stuck.
pub const STUCK_AFTER: Duration = Duration::hours(1);

/// A consumer's note on an action entry: it picked the entry up, with the
/// record it made (`result`), or, with `rejected`, that Daniel rejected
/// that result. Persisted in `pickups.json` (format 8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pickup {
    /// The entry it is about.
    pub entry: EntryId,
    /// The consumer, always an agent.
    pub author: Author,
    /// When it was recorded.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// What the consumer made of it (`task:<id>`), or `null`.
    #[serde(deserialize_with = "crate::record::nullable")]
    pub result: Option<Related>,
    /// Daniel rejected the result.
    pub rejected: bool,
}

/// Where an action entry stands for an intake, at one instant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionState {
    /// How long ago it was created.
    pub age: Duration,
    /// When its cancel window ends: `created` + [`CANCEL_WINDOW`] for an
    /// agent-written entry; `None` for Daniel's own entries.
    pub cancel_window_ends: Option<OffsetDateTime>,
    /// Whether an intake may take it now: past its cancel window (always,
    /// for Daniel's).
    pub past_cancel_window: bool,
    /// Whether any consumer recorded a pickup.
    pub picked_up: bool,
    /// Whether a consumer recorded that its result was rejected.
    pub rejected: bool,
    /// The pickups recorded, oldest first.
    pub pickups: Vec<Pickup>,
}

/// The state of `entry` at `now`, given every pickup in the store.
#[must_use]
pub fn state(entry: &Entry, pickups: &[Pickup], now: OffsetDateTime) -> ActionState {
    let cancel_window_ends =
        (entry.author.kind == AuthorKind::Agent).then(|| entry.created + CANCEL_WINDOW);
    let mut mine: Vec<Pickup> = pickups
        .iter()
        .filter(|p| p.entry == entry.id)
        .cloned()
        .collect();
    mine.sort_by_key(|p| p.at);
    ActionState {
        age: now - entry.created,
        past_cancel_window: cancel_window_ends.is_none_or(|ends| now >= ends),
        cancel_window_ends,
        picked_up: !mine.is_empty(),
        rejected: mine.iter().any(|p| p.rejected),
        pickups: mine,
    }
}

/// Active action entries older than [`STUCK_AFTER`] at `now` that no
/// consumer has picked up, counted per tag. An idea the session pass
/// captured itself is not counted: nobody asked for it to be taken up
/// (2_design.md §11.6).
#[must_use]
pub fn stuck(
    entries: &[Entry],
    tags: &TagTable,
    pickups: &[Pickup],
    now: OffsetDateTime,
) -> BTreeMap<TagName, usize> {
    let mut counts = BTreeMap::new();
    for entry in entries {
        if entry.status == EntryStatus::Active
            && tags.action(&entry.tag).is_some()
            && !crate::ask::is_pass_captured(entry)
            && now - entry.created >= STUCK_AFTER
            && !pickups.iter().any(|p| p.entry == entry.id)
        {
            *counts.entry(entry.tag.clone()).or_default() += 1;
        }
    }
    counts
}

/// Per action tag: `(retired, rejected)`, the agent-written entries under it
/// that were retired or forgotten, and those whose result was rejected.
#[must_use]
pub fn tag_counts(
    entries: &[Entry],
    tags: &TagTable,
    pickups: &[Pickup],
) -> BTreeMap<TagName, (usize, usize)> {
    let mut counts: BTreeMap<TagName, (usize, usize)> = BTreeMap::new();
    for entry in entries {
        if entry.author.kind != AuthorKind::Agent || tags.action(&entry.tag).is_none() {
            continue;
        }
        let count = counts.entry(entry.tag.clone()).or_default();
        if matches!(entry.status, EntryStatus::Retired | EntryStatus::Forgotten) {
            count.0 += 1;
        }
        if pickups.iter().any(|p| p.entry == entry.id && p.rejected) {
            count.1 += 1;
        }
    }
    counts
}

/// Whether `quote` is a verbatim span ([`verbatim::contains`]) of Daniel's
/// own words in one of the typed turns of `messages`: user turns only,
/// pasted blocks, harness wrappers and `/scribe` lines left out.
#[must_use]
pub fn quote_found(messages: &[Message], quote: &str) -> bool {
    user_turns(messages)
        .iter()
        .any(|turn| verbatim::contains(&turn.own, quote))
}
