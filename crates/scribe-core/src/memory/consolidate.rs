//! The consistency scan's memory steps (docs/design/scribe/2_design.md
//! §13.5), after the entry steps and before the session steps:
//!
//! 1. [`candidates`], model-free: within one section (and one project for
//!    `projects`), active items whose statements overlap by at least one
//!    content token in four; newest pairs first, at most
//!    [`crate::consistency::MAX_PAIRS`]. A pair judged before is never
//!    offered again: `same` and `contradicts` leave an item no longer
//!    active, and `unrelated` is kept on the older item
//!    ([`super::MemoryItem::unrelated_to`]);
//! 2. the `consistency` model labels each pair `same`, `contradicts` or
//!    `unrelated` ([`request`]), [`crate::consistency::BATCH`] at a time;
//! 3. [`judge`] keeps a `same` or `contradicts` label only when each quote
//!    is verbatim in one of that side's stored quotes;
//! 4. [`apply`] merges (`same`: the newer item's occurrences move onto the
//!    older, which keeps its id) and supersedes (`contradicts`: the older
//!    item is superseded by the newer). The order is the pair's, never the
//!    model's.
//!
//! Unlike the entry steps, these change items directly: memory is
//! descriptive and scribe-authored, and every change is reversible by
//! reading the occurrences (a merge moves them, a supersede keeps both).
//!
//! Pure functions over loaded items: no I/O (STD-02 R5).

use super::{MemoryId, MemoryItem, MemoryStatus, Section, overlaps_quarter, token_set};
use crate::models::{
    CONSISTENCY_BUDGET, ConsistencyAnswer, MemoryPairItem, MemoryPairSide, MemoryPairsRequest,
};
use crate::record::Author;
use crate::verbatim;
use std::collections::BTreeSet;
use time::OffsetDateTime;

/// Two active items of one section (and project) whose statements
/// overlap, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    /// The item written first: the one a merge keeps and a supersede
    /// replaces.
    pub older: MemoryItem,
    /// The item written later.
    pub newer: MemoryItem,
}

/// What a scan offers the model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Candidates {
    /// Candidate pairs, newest first, at most the cap.
    pub pairs: Vec<Pair>,
    /// Candidate pairs the cap left for a later scan.
    pub over_cap: usize,
}

/// The candidate pairs among `items`, newest first, at most `max_pairs`.
#[must_use]
pub fn candidates(items: &[MemoryItem], max_pairs: usize) -> Candidates {
    let active: Vec<(&MemoryItem, BTreeSet<String>)> = items
        .iter()
        .filter(|i| i.status == MemoryStatus::Active)
        .map(|i| (i, token_set(&i.statement)))
        .collect();
    let mut pairs = Vec::new();
    for (i, (a, a_tokens)) in active.iter().enumerate() {
        for (b, b_tokens) in active.iter().skip(i.saturating_add(1)) {
            if a.section != b.section
                || (a.section == Section::Projects && a.project != b.project)
                || !overlaps_quarter(a_tokens, b_tokens)
            {
                continue;
            }
            let (older, newer) = ordered(a, b);
            if older.unrelated_to.contains(&newer.id) {
                continue;
            }
            pairs.push(Pair {
                older: older.clone(),
                newer: newer.clone(),
            });
        }
    }
    let key = |p: &Pair| {
        (
            p.newer.created,
            p.newer.id.number(),
            p.older.created,
            p.older.id.number(),
        )
    };
    pairs.sort_by_key(|p| std::cmp::Reverse(key(p)));
    let over_cap = pairs.len().saturating_sub(max_pairs);
    pairs.truncate(max_pairs);
    Candidates { pairs, over_cap }
}

/// The older and the newer of two items: by `created`, then id.
fn ordered<'a>(a: &'a MemoryItem, b: &'a MemoryItem) -> (&'a MemoryItem, &'a MemoryItem) {
    if (a.created, a.id.number()) <= (b.created, b.id.number()) {
        (a, b)
    } else {
        (b, a)
    }
}

/// The model request for one batch of pairs, numbered from 1.
#[must_use]
pub fn request(model: &str, pairs: &[Pair], voice: crate::voice::Voice) -> MemoryPairsRequest {
    let side = |i: &MemoryItem| MemoryPairSide {
        id: i.id.to_string(),
        date: i.latest().date().to_string(),
        statement: i.statement.clone(),
        quotes: quotes(i).map(str::to_owned).collect(),
    };
    MemoryPairsRequest {
        voice,
        model: model.to_owned(),
        budget: CONSISTENCY_BUDGET,
        pairs: pairs
            .iter()
            .zip(1u32..)
            .map(|(pair, n)| MemoryPairItem {
                pair: n.to_string(),
                section: pair.older.section.to_string(),
                older: side(&pair.older),
                newer: side(&pair.newer),
            })
            .collect(),
    }
}

fn quotes(item: &MemoryItem) -> impl Iterator<Item = &str> {
    item.occurrences
        .iter()
        .map(|o| o.quote.as_str())
        .filter(|q| !q.trim().is_empty())
}

/// A pair's surviving label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    /// Both say the same thing: merge.
    Same,
    /// The newer replaces the older: supersede.
    Contradicts,
    /// Both hold: never offered again.
    Unrelated,
}

/// One surviving label, by id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judgement {
    /// The older item.
    pub older: MemoryId,
    /// The newer item.
    pub newer: MemoryId,
    /// The label.
    pub label: Label,
}

/// What [`judge`] made of one answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Judged {
    /// The surviving labels, in pair order.
    pub judgements: Vec<Judgement>,
    /// Labels dropped: a quote not verbatim in its side's quotes, an
    /// unknown pair or label, or a second label for the same pair.
    pub dropped: usize,
}

/// Check the model's labels for `pairs` (the batch the request numbered):
/// `same` and `contradicts` survive only when `older_quote` is verbatim in
/// one of the older item's quotes and `newer_quote` in one of the newer's;
/// `unrelated` needs no quote.
#[must_use]
pub fn judge(answer: ConsistencyAnswer, pairs: &[Pair]) -> Judged {
    let mut out = Judged::default();
    let mut seen = BTreeSet::new();
    let mut found: Vec<(usize, Judgement)> = Vec::new();
    for j in answer.judgements {
        let index = j
            .pair
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|i| *i < pairs.len());
        let Some(index) = index.filter(|i| seen.insert(*i)) else {
            out.dropped = out.dropped.saturating_add(1);
            continue;
        };
        let pair = &pairs[index];
        let label = match j.label.trim() {
            "unrelated" => Label::Unrelated,
            "same" | "contradicts" => {
                let older_quote = j.older_quote.trim();
                let newer_quote = j.newer_quote.trim();
                let verbatim_in = |item: &MemoryItem, quote: &str| {
                    !quote.is_empty() && quotes(item).any(|q| verbatim::contains(q, quote))
                };
                if !verbatim_in(&pair.older, older_quote) || !verbatim_in(&pair.newer, newer_quote)
                {
                    out.dropped = out.dropped.saturating_add(1);
                    continue;
                }
                if j.label.trim() == "same" {
                    Label::Same
                } else {
                    Label::Contradicts
                }
            }
            _ => {
                out.dropped = out.dropped.saturating_add(1);
                continue;
            }
        };
        found.push((
            index,
            Judgement {
                older: pair.older.id.clone(),
                newer: pair.newer.id.clone(),
                label,
            },
        ));
    }
    found.sort_by_key(|(i, _)| *i);
    out.judgements = found.into_iter().map(|(_, j)| j).collect();
    out
}

/// One change [`apply`] made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// `item` merged into `into`.
    Merged {
        /// The newer item, now `merged`.
        item: MemoryId,
        /// The older item that holds its occurrences.
        into: MemoryId,
    },
    /// `item` superseded by `by`.
    Superseded {
        /// The older item, now `superseded`.
        item: MemoryId,
        /// The newer item.
        by: MemoryId,
    },
    /// The pair judged unrelated, kept on `older`.
    Unrelated {
        /// The older item.
        older: MemoryId,
        /// The newer item.
        newer: MemoryId,
    },
}

/// Apply the surviving labels to `items`, as `by` at `now`. A label whose
/// items are no longer both active (an earlier label in the same scan
/// changed one) is skipped, so applying the same labels again changes
/// nothing.
pub fn apply(
    items: &mut [MemoryItem],
    judgements: &[Judgement],
    by: &Author,
    now: OffsetDateTime,
) -> Vec<Change> {
    let mut changes = Vec::new();
    for j in judgements {
        let position = |id: &MemoryId| {
            items
                .iter()
                .position(|i| &i.id == id && i.status == MemoryStatus::Active)
        };
        let (Some(older), Some(newer)) = (position(&j.older), position(&j.newer)) else {
            continue;
        };
        match j.label {
            Label::Same => {
                merge(items, older, newer, by, now);
                changes.push(Change::Merged {
                    item: j.newer.clone(),
                    into: j.older.clone(),
                });
            }
            Label::Contradicts => {
                let old = &mut items[older];
                old.status = MemoryStatus::Superseded;
                old.superseded_by = Some(j.newer.clone());
                old.updated = now;
                old.updated_by = by.clone();
                changes.push(Change::Superseded {
                    item: j.older.clone(),
                    by: j.newer.clone(),
                });
            }
            Label::Unrelated => {
                let old = &mut items[older];
                if old.unrelated_to.contains(&j.newer) {
                    continue;
                }
                old.unrelated_to.push(j.newer.clone());
                old.updated = now;
                old.updated_by = by.clone();
                changes.push(Change::Unrelated {
                    older: j.older.clone(),
                    newer: j.newer.clone(),
                });
            }
        }
    }
    changes
}

/// Move every occurrence of `items[from]` onto `items[into]`, numbered
/// past its highest and keeping each one's author, newest first (a turn
/// the target already holds is not added twice); `from` becomes `merged`.
pub fn merge(items: &mut [MemoryItem], into: usize, from: usize, by: &Author, now: OffsetDateTime) {
    let moved = std::mem::take(&mut items[from].occurrences);
    let (event_date, sensitive, sensitive_kind) = {
        let f = &items[from];
        (f.event_date, f.sensitive, f.sensitive_kind)
    };
    let target_id = items[into].id.clone();
    {
        let f = &mut items[from];
        f.status = MemoryStatus::Merged;
        f.merged_into = Some(target_id);
        f.updated = now;
        f.updated_by = by.clone();
    }
    let target = &mut items[into];
    let mut n = target.occurrences.iter().map(|o| o.n).max().unwrap_or(0);
    for mut occurrence in moved {
        // A crash between the two files leaves the target holding them
        // already: a turn is never recorded twice.
        if target
            .occurrences
            .iter()
            .any(|o| o.message == occurrence.message && o.session == occurrence.session)
        {
            continue;
        }
        n = n.saturating_add(1);
        occurrence.n = n;
        target.occurrences.push(occurrence);
    }
    target
        .occurrences
        .sort_by(|a, b| b.date.cmp(&a.date).then(b.n.cmp(&a.n)));
    if target.event_date.is_none() {
        target.event_date = event_date;
    }
    if sensitive && !target.sensitive {
        target.sensitive = true;
        target.sensitive_kind = sensitive_kind;
    }
    target.updated = now;
    target.updated_by = by.clone();
}
