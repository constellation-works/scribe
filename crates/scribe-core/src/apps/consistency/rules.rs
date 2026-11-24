//! The consistency scan's stale step (docs/design/scribe/2_design.md §7):
//! which entries are due for review, and the `stale` draft for each. Pure
//! functions over loaded records: no I/O (STD-02 R5).

use crate::consistency::Draft;
use crate::entry::{Entry, EntryStatus};
use crate::proposal::{Proposal, ProposalKind, Quote};
use crate::verbatim;
use time::Date;

/// The longest quote of a stale entry a proposal keeps, in characters.
pub const STALE_QUOTE_CHARS: usize = 200;

/// The scan's stale step (§7): the active entries whose `review_after` is
/// `today` or earlier, without a stale proposal in any state, oldest first.
#[must_use]
pub fn stale_candidates(entries: &[Entry], proposals: &[Proposal], today: Date) -> Vec<Entry> {
    let mut stale: Vec<Entry> = entries
        .iter()
        .filter(|e| e.status == EntryStatus::Active)
        .filter(|e| e.review_after.is_some_and(|d| d <= today))
        .filter(|e| {
            !proposals
                .iter()
                .any(|p| p.kind == ProposalKind::Stale && p.older == e.id)
        })
        .cloned()
        .collect();
    stale.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    stale
}

/// The `stale` draft for an entry past its `review_after`. The reason is
/// scribe's own sentence, not a model's; the quote is the start of the
/// entry's content, when it is long enough to pass the verbatim check.
#[must_use]
pub fn stale(entry: &Entry) -> Draft {
    let due = entry
        .review_after
        .map_or_else(String::new, |d| format!(" on {d}"));
    let start = cut(entry.content.trim(), STALE_QUOTE_CHARS);
    let quotes = if verbatim::contains(&entry.content, &start) {
        vec![Quote {
            entry: entry.id.clone(),
            text: start,
        }]
    } else {
        Vec::new()
    };
    Draft {
        kind: ProposalKind::Stale,
        older: entry.id.clone(),
        newer: None,
        reason: format!(
            "{} was due for review{due}; approve to retire it, reject to keep it",
            entry.id
        ),
        quotes,
    }
}

/// `text`'s first `max` characters.
fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
