//! The migration of format-24 `memory/m-<n>.md` items into the fixed shape
//! (docs/design/scribe/2_design.md §15.11). No I/O: [`plan`] says what each
//! item becomes, and the store writes it through the memory journal on the
//! first memory write.
//!
//! - An active `now` item dated in the future, or under
//!   [`super::NOW_KEEP_DAYS`] days past, becomes a `now` file; an active
//!   `milestones` item becomes a milestone file.
//! - `profile` and `projects` items become hints for the first refresh,
//!   kept as their `memory_history` rows.
//! - An active `working` item is offered to the store, which opens a
//!   `preferences` claim from it when its quote fires that tag's capture
//!   trigger (a standing rule, as `m-1`), and otherwise keeps it in history
//!   only (as `m-3`).
//! - Every item's statement and quotes go to `memory_history`, except a
//!   forgotten item's, which hold no text.

use super::legacy::{MemoryItem, MemoryStatus, Section};
use super::{Milestone, NowItem, NowKind, Raised, Stem, TEXT_CHARS, TITLE_CHARS, expired};
use time::{Date, OffsetDateTime};

/// What the migration writes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Migration {
    /// The `now` files to write.
    pub now: Vec<NowItem>,
    /// The milestone files to write.
    pub milestones: Vec<Milestone>,
    /// One history row per item that holds text: (id, section, text,
    /// sessions).
    pub history: Vec<(String, &'static str, String, Vec<String>)>,
    /// The active `working` items, for the store's preference check.
    pub working: Vec<MemoryItem>,
    /// Every `m-<n>` file, to remove.
    pub remove: Vec<String>,
}

/// A title for a migrated item: its statement cut at [`TITLE_CHARS`] on a
/// word, with `…` when cut.
fn title(statement: &str) -> String {
    if statement.chars().count() <= TITLE_CHARS {
        return statement.to_owned();
    }
    let mut out = String::new();
    for word in statement.split_whitespace() {
        if out.chars().count() + word.chars().count() + 2 > TITLE_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out.push('…');
    out
}

fn text(statement: &str) -> String {
    statement.chars().take(TEXT_CHARS).collect()
}

fn sources(item: &MemoryItem) -> Vec<crate::session::SessionId> {
    let mut out: Vec<crate::session::SessionId> = Vec::new();
    for o in &item.occurrences {
        if !out.contains(&o.session) {
            out.push(o.session.clone());
        }
    }
    out
}

/// The history text of an item: its statement, then each quote as a `> `
/// line.
fn history_text(item: &MemoryItem) -> String {
    let mut out = item.statement.clone();
    for o in item.occurrences.iter().filter(|o| !o.quote.is_empty()) {
        out.push_str("\n> ");
        out.push_str(&o.quote.replace('\n', " "));
    }
    out
}

/// What `items` become on `today`.
#[must_use]
pub fn plan(items: &[MemoryItem], today: Date) -> Migration {
    let mut out = Migration::default();
    for item in items {
        out.remove.push(item.id.to_string());
        if item.status == MemoryStatus::Forgotten {
            continue;
        }
        let sessions = sources(item);
        out.history.push((
            item.id.to_string(),
            item.section.as_str(),
            history_text(item),
            sessions.iter().map(ToString::to_string).collect(),
        ));
        if item.status != MemoryStatus::Active {
            continue;
        }
        let updated: OffsetDateTime = item.updated;
        match (item.section, item.event_date) {
            (Section::Now, Some(date)) if !expired(date, today) && !sessions.is_empty() => {
                out.now.push(NowItem {
                    stem: Stem::new(date, &item.statement),
                    date,
                    kind: NowKind::Plan,
                    sources: sessions,
                    updated,
                    raised: item.acknowledged.as_ref().map(|a| Raised {
                        at: a.at,
                        session: a.session.clone(),
                    }),
                    title: title(&item.statement),
                    text: text(&item.statement),
                });
            }
            (Section::Milestones, date) if !sessions.is_empty() => {
                let date = date.unwrap_or_else(|| item.created.date());
                out.milestones.push(Milestone {
                    stem: Stem::new(date, &item.statement),
                    date,
                    sources: sessions,
                    updated,
                    title: title(&item.statement),
                    text: text(&item.statement),
                });
            }
            (Section::Working, _) => out.working.push(item.clone()),
            _ => {}
        }
    }
    out
}
