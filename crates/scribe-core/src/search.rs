//! `record search`: the store's records ranked for a few words, with no
//! model and no Orbit call (docs/design/scribe/2_design.md §6.8).
//!
//! [`search`] runs `ask`'s retrieval over the records the caller loaded and
//! stops there: the same units (each entry, each session header and each
//! segment, §6.7), the same tokenizer, weights and BM25 ranking
//! ([`crate::ask`]), with no lookup classifier, no tasks and no answer. The
//! filters narrow the units before ranking; hidden entries and inactive
//! sessions are ranked either way and kept only when asked for, so the
//! caller can say how many it left out. Forgotten entries never match.
//!
//! No I/O (STD-02 R5).

use crate::ask::{self, Corpus, Doc, Origin, RecordKind};
use crate::entry::Entry;
use crate::record::{Author, DateRange, Project, Topic};
use crate::session::{Segment, SessionRecord};
use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use time::OffsetDateTime;

/// How many hits a search returns unless told otherwise.
pub const DEFAULT_LIMIT: usize = 20;
/// The longest snippet, in characters, ellipses included.
pub const SNIPPET_CHARS: usize = 240;
/// How much of the snippet comes before the first matching word.
const LEAD_CHARS: usize = 60;

/// Why a search was refused before it ran.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
    /// Nothing in the terms is a word ranking can use.
    #[error(
        "no searchable word in {0:?}: give a word of at least three letters that is not a stopword such as 'the' or 'about'"
    )]
    NoTerms(String),
}

/// A search and the filters that narrow it. Every filter set must match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    /// The words to look for.
    pub terms: String,
    /// Only this kind of record.
    pub kind: Option<RecordKind>,
    /// Only records of this project.
    pub project: Option<Project>,
    /// Only units carrying every one of these topics (a segment carries its
    /// record's and its own).
    pub topics: Vec<Topic>,
    /// Only entries created, sessions and segments running, on a day in
    /// this range.
    pub dates: DateRange,
    /// Superseded and retired entries and inactive sessions too.
    pub include_hidden: bool,
    /// At most this many hits.
    pub limit: NonZeroUsize,
}

/// One ranked unit.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// An entry or a session (its header or one segment).
    pub kind: RecordKind,
    /// What `ask` would cite: an entry id, `session:<id>` or
    /// `session:<id>#<n>`.
    pub cite: String,
    /// The entry's or the session's id.
    pub id: String,
    /// The segment's number, for a segment.
    pub segment: Option<u32>,
    /// The BM25 score times the unit's weight, to three decimals.
    pub score: f64,
    /// The session's title, the segment's heading, or for an entry its id
    /// and tag (`preferences-2 (preferences)`).
    pub title: String,
    /// Up to [`SNIPPET_CHARS`] characters around the first matching word.
    pub snippet: String,
    /// The record's project.
    pub project: Project,
    /// The unit's topics: an entry's, a session's, and a segment's own too.
    pub topics: Vec<Topic>,
    /// An entry's author; `None` for a session.
    pub author: Option<Author>,
    /// Whether the session is active; `None` for an entry, whose `status`
    /// says whether it is hidden.
    pub active: Option<bool>,
    /// The entry's status (`active`, `superseded`, `retired`) or the
    /// session's (`open`, `final`).
    pub status: &'static str,
    /// When the entry or session last changed, or the segment's last message.
    pub updated: OffsetDateTime,
    /// A finding entry's evidence pointer; `None` otherwise.
    pub evidence: Option<String>,
    /// The date a finding's result was true; `None` otherwise.
    pub as_of: Option<time::Date>,
}

/// What a search found.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The hits, best first, cut to the limit.
    pub hits: Vec<Hit>,
    /// How many units matched before the limit.
    pub total: usize,
    /// How many hidden records also matched and were left out.
    pub hidden_matches: usize,
}

/// Rank the corpus for `query`.
///
/// # Errors
///
/// [`SearchError::NoTerms`] when the terms hold no word ranking can use.
pub fn search(query: &SearchQuery, corpus: Corpus<'_>) -> Result<Found, SearchError> {
    let wanted: BTreeSet<String> = ask::tokens(&query.terms).into_iter().collect();
    if wanted.is_empty() {
        return Err(SearchError::NoTerms(query.terms.clone()));
    }
    let words = ask::words(&query.terms);
    let docs = ask::store_docs(
        &words,
        corpus,
        |e| keeps_entry(query, e),
        |r, s| keeps_unit(query, r, s),
    );
    let mut ranked = ask::rank(&words, corpus, docs, &wanted);
    let hidden_matches = if query.include_hidden {
        0
    } else {
        ask::hidden_records(&ranked)
    };
    ranked.retain(|(_, d)| query.include_hidden || !d.hidden);
    let total = ranked.len();
    let hits = ranked
        .iter()
        .take(query.limit.get())
        .filter_map(|(score, doc)| hit(*score, doc, corpus, &wanted))
        .collect();
    Ok(Found {
        hits,
        total,
        hidden_matches,
    })
}

fn keeps_entry(query: &SearchQuery, entry: &Entry) -> bool {
    query.kind.is_none_or(|k| k == RecordKind::Entry)
        && query.project.as_ref().is_none_or(|p| &entry.project == p)
        && query.topics.iter().all(|t| entry.topics.contains(t))
        && query.dates.contains(entry.created)
}

fn keeps_unit(query: &SearchQuery, record: &SessionRecord, segment: Option<&Segment>) -> bool {
    let kind_and_project = query.kind.is_none_or(|k| k == RecordKind::Session)
        && query.project.as_ref().is_none_or(|p| &record.project == p);
    kind_and_project
        && match segment {
            None => {
                query.topics.iter().all(|t| record.topics.contains(t))
                    && query.dates.overlaps(record.started, record.updated)
            }
            Some(s) => {
                query
                    .topics
                    .iter()
                    .all(|t| record.topics.contains(t) || s.topics.contains(t))
                    && query.dates.overlaps(s.from, s.to)
            }
        }
}

fn hit(score: f64, doc: &Doc, corpus: Corpus<'_>, wanted: &BTreeSet<String>) -> Option<Hit> {
    let score = (score * 1000.0).round() / 1000.0;
    let cite = doc.item.cite.clone();
    match doc.origin {
        Origin::Entry(i) => {
            let e = corpus.entries.get(i)?;
            let text = if e.content.trim().is_empty() {
                &e.said
            } else {
                &e.content
            };
            // A finding reads as that agent's claim as of its date, with
            // its evidence pointer, in every output mode.
            let snippet = if e.author.kind == crate::AuthorKind::Agent && e.is_finding() {
                format!(
                    "{} [agent claim ({}) as of {}; evidence: {}]",
                    snippet(text, wanted),
                    e.author.name,
                    e.as_of.map_or_else(
                        || crate::record::DateRange::day(e.created.date()),
                        crate::record::DateRange::day
                    ),
                    e.evidence.as_deref().unwrap_or_default()
                )
            } else if let (Some(quote), Some(outcome)) = (ask::stated_quote(e), &e.outcome) {
                // An idea the pass captured: scribe's statement, Daniel's
                // own words and where it stands (2_design.md §11.6).
                format!(
                    "{}: {} | {} \"{}\" | outcome: {}",
                    ask::CAPTURED_WHOSE,
                    snippet(text, wanted),
                    ask::STATED_QUOTE_LABEL,
                    snippet(quote, wanted),
                    ask::outcome_text(outcome)
                )
            } else if let Some(quote) = ask::stated_quote(e) {
                // Scribe's statement of Daniel's turn, then his own words,
                // labelled as his (2_design.md §11.1).
                format!(
                    "{}: {} | {} \"{}\"",
                    ask::STATED_WHOSE,
                    snippet(text, wanted),
                    ask::STATED_QUOTE_LABEL,
                    snippet(quote, wanted)
                )
            } else {
                snippet(text, wanted)
            };
            Some(Hit {
                kind: RecordKind::Entry,
                cite,
                id: e.id.to_string(),
                segment: None,
                score,
                title: format!("{} ({})", e.id, e.tag),
                snippet,
                project: e.project.clone(),
                topics: e.topics.as_slice().to_vec(),
                author: Some(e.author.clone()),
                active: None,
                status: e.status.as_str(),
                updated: e.updated,
                evidence: e.evidence.clone(),
                as_of: e.as_of,
            })
        }
        Origin::Header(i) => {
            let r = corpus.sessions.get(i)?;
            let text = format!("{}\n{}\n{}", r.summary, r.goals, r.next);
            Some(Hit {
                kind: RecordKind::Session,
                cite,
                id: r.id.to_string(),
                segment: None,
                score,
                title: r.title.clone(),
                snippet: snippet(&text, wanted),
                project: r.project.clone(),
                topics: r.topics.as_slice().to_vec(),
                author: None,
                active: Some(r.active),
                status: r.status.as_str(),
                updated: r.updated,
                evidence: None,
                as_of: None,
            })
        }
        Origin::Segment(i, j) => {
            let r = corpus.sessions.get(i)?;
            let s = r.log.get(j)?;
            let mut topics = r.topics.as_slice().to_vec();
            for t in s.topics.as_slice() {
                if !topics.contains(t) {
                    topics.push(t.clone());
                }
            }
            Some(Hit {
                kind: RecordKind::Session,
                cite,
                id: r.id.to_string(),
                segment: Some(s.n.get()),
                score,
                title: s.heading.clone(),
                snippet: snippet(&s.text, wanted),
                project: r.project.clone(),
                topics,
                author: None,
                active: Some(r.active),
                status: r.status.as_str(),
                updated: s.to,
                evidence: None,
                as_of: None,
            })
        }
        Origin::Task => None,
    }
}

fn flatten(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Up to [`SNIPPET_CHARS`] characters of `text` on one line, starting a
/// little before the first word whose token is wanted, with an ellipsis
/// where it was cut.
#[must_use]
pub fn snippet(text: &str, wanted: &BTreeSet<String>) -> String {
    let flat = flatten(text);
    let words: Vec<&str> = flat.split(' ').collect();
    let mut offset = 0usize;
    let mut first = None;
    for word in &words {
        if ask::tokens(word).iter().any(|t| wanted.contains(t)) {
            first = Some(offset);
            break;
        }
        offset += word.chars().count() + 1;
    }
    let total = flat.chars().count();
    if total <= SNIPPET_CHARS {
        return flat;
    }
    let mut start = first.map_or(0, |at| at.saturating_sub(LEAD_CHARS));
    // Never so late that the window is short, then forward to a word's
    // start, which keeps the end of the text in the window.
    start = start.min(total.saturating_sub(SNIPPET_CHARS - 1));
    let chars: Vec<char> = flat.chars().collect();
    while start > 0 && start < total && chars.get(start - 1).is_some_and(|c| *c != ' ') {
        start += 1;
    }
    let lead = usize::from(start > 0);
    let room = SNIPPET_CHARS - lead;
    let end = (start + room).min(total);
    let tail = usize::from(end < total);
    let end = end - tail;
    let mut out = String::new();
    if lead == 1 {
        out.push('…');
    }
    out.extend(chars.get(start..end).unwrap_or_default());
    if tail == 1 {
        let trimmed = out.trim_end().len();
        out.truncate(trimmed);
        out.push('…');
    }
    out
}
