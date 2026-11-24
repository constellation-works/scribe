//! `scribe search`: the store's records ranked for a few words, with no
//! model and no Orbit call (docs/design/scribe/2_design.md §6.8).
//!
//! [`search`] ranks the records the caller loaded: the units (each entry,
//! each session header and each segment, §6.7), the tokenizer
//! ([`crate::text::tokens`]), the weights and the BM25 ranking below. The
//! filters narrow the units before ranking; hidden entries and inactive
//! sessions are ranked either way and kept only when asked for, so the
//! caller can say how many it left out. Forgotten entries never match.
//!
//! [`messages`] ranks nothing itself: the store's FTS5 `bm25()` already
//! ordered the stored messages (2_design.md §15.7), and it applies the same
//! filters through each message's session record or group provenance. Message
//! scores are not comparable with the other kinds', so messages are searched only as
//! `--kind message`.
//!
//! No I/O (STD-02 R5).

use crate::entry::{ContentFrom, Entry, EntryStatus, Speaker, TagName};
use crate::message::MessageSearch;
use crate::record::{Author, DateRange, Project, Topic};
use crate::session::{Segment, SessionRecord};
use crate::tag::TagTable;
use crate::text::tokens;
use crate::voice::{
    INSTRUCTION_YOURS, UserName, approved_line, captured_whose, day, instruction_theirs,
    outcome_text, speaker_of, stated_quote, stated_whose_of, words_label,
};
use std::collections::{BTreeMap, BTreeSet};
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
    /// Only entries filed under this tag, by its exact name as `entry list`
    /// matches it. Sessions and messages carry no tag, so a set tag keeps
    /// none of them.
    pub tag: Option<TagName>,
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
    /// Its cite: an entry id, `session:<id>` or
    /// `session:<id>#<n>`.
    pub cite: String,
    /// The entry's or the session's id.
    pub id: String,
    /// The segment's number, for a segment.
    pub segment: Option<u32>,
    /// The provider's message id, for a stored message.
    pub message: Option<String>,
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
    let wanted: BTreeSet<String> = tokens(&query.terms).into_iter().collect();
    if wanted.is_empty() {
        return Err(SearchError::NoTerms(query.terms.clone()));
    }
    let words = words(&query.terms);
    let docs = store_docs(
        &words,
        corpus,
        |e| keeps_entry(query, e),
        |r, s| keeps_unit(query, r, s),
    );
    let mut ranked = rank(&words, corpus, docs, &wanted);
    let hidden_matches = if query.include_hidden {
        0
    } else {
        hidden_records(&ranked)
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

/// The stored messages of `found` that `query`'s filters keep, as hits,
/// best first. A session record supplies project and topics when present;
/// otherwise group-only messages use their group's project and have no topics.
/// Dates use the message's time. An inactive session's messages are hidden
/// unless `include_hidden`.
///
/// # Errors
///
/// [`SearchError::NoTerms`] when the terms hold no word ranking can use.
pub fn messages(
    query: &SearchQuery,
    found: &MessageSearch,
    sessions: &[SessionRecord],
) -> Result<Found, SearchError> {
    let wanted: BTreeSet<String> = tokens(&query.terms).into_iter().collect();
    if wanted.is_empty() {
        return Err(SearchError::NoTerms(query.terms.clone()));
    }
    let mut kept = Vec::new();
    let mut hidden_matches = 0;
    for m in &found.matches {
        let record = sessions.iter().find(|r| r.id == m.session);
        let Some(project) = record.map(|r| &r.project).or(m.group_project.as_ref()) else {
            continue;
        };
        let keeps = query.tag.is_none()
            && query.project.as_ref().is_none_or(|p| project == p)
            && query
                .topics
                .iter()
                .all(|t| record.is_some_and(|r| r.topics.contains(t)))
            && query.dates.contains(m.at);
        if !keeps {
            continue;
        }
        if record.is_some_and(|r| !r.active) && !query.include_hidden {
            hidden_matches += 1;
            continue;
        }
        kept.push((m, record, project));
    }
    let total = kept.len();
    let hits = kept
        .into_iter()
        .take(query.limit.get())
        .map(|(m, record, project)| Hit {
            kind: RecordKind::Message,
            cite: format!("session:{}", m.session),
            id: m.session.to_string(),
            segment: None,
            message: Some(m.message.clone()),
            score: (m.score * 1000.0).round() / 1000.0,
            title: format!(
                "{} ({})",
                record.map_or(m.session.as_str(), |r| r.title.as_str()),
                m.role.as_str()
            ),
            snippet: snippet(&m.text, &wanted),
            project: project.clone(),
            topics: record.map_or_else(Vec::new, |r| r.topics.as_slice().to_vec()),
            author: None,
            active: record.map(|r| r.active),
            status: record.map_or("stored", |r| r.status.as_str()),
            updated: m.at,
            evidence: None,
            as_of: None,
        })
        .collect();
    Ok(Found {
        hits,
        total,
        hidden_matches,
    })
}

fn keeps_entry(query: &SearchQuery, entry: &Entry) -> bool {
    query.kind.is_none_or(|k| k == RecordKind::Entry)
        && query.tag.as_ref().is_none_or(|t| &entry.tag == t)
        && query.project.as_ref().is_none_or(|p| &entry.project == p)
        && query.topics.iter().all(|t| entry.topics.contains(t))
        && query.dates.contains(entry.created)
}

fn keeps_unit(query: &SearchQuery, record: &SessionRecord, segment: Option<&Segment>) -> bool {
    // A session carries no tag, so a set tag keeps no session or segment.
    let kind_and_project = query.tag.is_none()
        && query.kind.is_none_or(|k| k == RecordKind::Session)
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
            } else if let (Some(quote), Some(outcome)) = (stated_quote(e), &e.outcome) {
                // An entry the pass captured: scribe's statement, the
                // turn's own words and where it stands (2_design.md §11.6).
                format!(
                    "{}: {} | {} \"{}\" | outcome: {}",
                    captured_whose(corpus.user, speaker_of(e)),
                    snippet(text, wanted),
                    words_label(corpus.user, speaker_of(e)),
                    snippet(quote, wanted),
                    outcome_text(outcome, corpus.user)
                )
            } else if let Some(quote) = stated_quote(e) {
                // Scribe's or an agent's statement of the turn, then the
                // speaker's own words, labelled as theirs
                // (2_design.md §11.1, §15.3, §16.6).
                let speaker = speaker_of(e);
                format!(
                    "{}: {} | {} \"{}\"",
                    stated_whose_of(corpus.user, e),
                    snippet(text, wanted),
                    words_label(corpus.user, speaker),
                    snippet(quote, wanted)
                )
            } else {
                snippet(text, wanted)
            };
            let snippet = e.handoff_mode.map_or(snippet.clone(), |m| {
                format!("{snippet} | handoff mode: {}", m.as_str())
            });
            // An approval an agent made on his instruction says so after
            // the snippet, in every output mode (2_design.md §12.3).
            let snippet = match approved_line(e, INSTRUCTION_YOURS) {
                Some(line) => format!("{snippet} | {line}"),
                None => snippet,
            };
            Some(Hit {
                kind: RecordKind::Entry,
                cite,
                id: e.id.to_string(),
                segment: None,
                message: None,
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
                message: None,
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
                message: None,
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
        if tokens(word).iter().any(|t| wanted.contains(t)) {
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

// --- ranking (2_design.md §6.2, §6.7) ----------------------------------------

/// Entries are the user's explicit words, so they outrank summaries (§6.2).
const ENTRY_WEIGHT: f64 = 1.5;

/// An entry in a tag the question names ranks higher again.
const TAG_BOOST: f64 = 1.5;

/// BM25's term-frequency saturation.
const BM25_K1: f64 = 1.2;

/// BM25's length normalization.
const BM25_B: f64 = 0.75;

/// Which record kind a scope keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordKind {
    /// Entries only.
    Entry,
    /// Session records only.
    Session,
    /// Stored messages only (`scribe.db`, 2_design.md §15.7).
    Message,
}

impl RecordKind {
    /// The canonical token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::Session => "session",
            Self::Message => "message",
        }
    }
}

/// The records a search ranks, loaded by the caller.
#[derive(Clone, Copy, Debug)]
pub struct Corpus<'a> {
    /// Every entry, any status.
    pub entries: &'a [Entry],
    /// Every session record.
    pub sessions: &'a [SessionRecord],
    /// Stored message matches supplied for a group-scoped question.
    pub messages: &'a [crate::message::MessageMatch],
    /// The configured tags, for tag words in the question.
    pub tags: &'a TagTable,
    /// How evidence labels and answers name the user (`[user] name`).
    pub user: &'a UserName,
}

/// The question's words for the lookup classifier: lowercase, split on
/// anything but letters, digits and `-`.
pub(crate) fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether `word` names `project`: the whole slug or its last path segment
/// (`scribe` names `constellation-works/scribe`). `global` names nothing.
fn project_named(project: &Project, word: &str) -> bool {
    let slug = project.as_str().to_lowercase();
    slug != Project::GLOBAL
        && word.chars().count() >= 3
        && (slug == word || slug.rsplit('/').next() == Some(word))
}

/// Where a ranked unit came from, by index into the [`Corpus`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// `corpus.entries[i]`.
    Entry(usize),
    /// The header of `corpus.sessions[i]`.
    Header(usize),
    /// Segment `log[j]` of `corpus.sessions[i]`.
    Segment(usize, usize),
}

/// A candidate unit, ready to rank: an entry, a session header or segment.
pub(crate) struct Doc {
    /// The record it belongs to (a session's units share one), for counting
    /// hidden matches once per record.
    pub(crate) record: String,
    pub(crate) item: EvidenceItem,
    pub(crate) tokens: Vec<String>,
    pub(crate) weight: f64,
    pub(crate) order: u8,
    pub(crate) at: OffsetDateTime,
    pub(crate) hidden: bool,
    pub(crate) origin: Origin,
}

/// The store's units (§6.2 step 1, §6.7): every entry `keep_entry` accepts
/// (never a forgotten one), and each session header and segment
/// `keep_unit` accepts (`None` for the header, the segment otherwise),
/// weighted by kind; an entry in a tag the `words` name is
/// boosted.
pub(crate) fn store_docs(
    words: &[String],
    corpus: Corpus<'_>,
    keep_entry: impl Fn(&Entry) -> bool,
    keep_unit: impl Fn(&SessionRecord, Option<&Segment>) -> bool,
) -> Vec<Doc> {
    let named_tags: Vec<&TagName> = words
        .iter()
        .filter_map(|w| corpus.tags.resolve(w).map(|t| &t.name))
        .collect();
    let mut docs: Vec<Doc> = Vec::new();
    for (i, entry) in corpus.entries.iter().enumerate() {
        if entry.status == EntryStatus::Forgotten || !keep_entry(entry) {
            continue;
        }
        let boost = if named_tags.contains(&&entry.tag) {
            TAG_BOOST
        } else {
            1.0
        };
        docs.push(Doc {
            record: entry.id.to_string(),
            tokens: tokens(&format!(
                "{} {} {} {}",
                entry.content,
                stated_quote(entry).unwrap_or_default(),
                entry.said,
                entry.tag
            )),
            weight: if entry.author.kind == crate::AuthorKind::User {
                ENTRY_WEIGHT * boost
            } else {
                boost
            },
            order: 0,
            at: entry.updated,
            hidden: entry.status.is_hidden(),
            item: entry_item(entry, corpus.user),
            origin: Origin::Entry(i),
        });
    }
    for (i, record) in corpus.sessions.iter().enumerate() {
        let key = format!("session:{}", record.id);
        if keep_unit(record, None) {
            docs.push(Doc {
                record: key.clone(),
                tokens: tokens(&format!(
                    "{} {} {} {}",
                    record.title, record.summary, record.goals, record.next
                )),
                weight: 1.0,
                order: 1,
                at: record.updated,
                hidden: !record.active,
                item: session_item(record, corpus.user),
                origin: Origin::Header(i),
            });
        }
        for (j, segment) in record.log.iter().enumerate() {
            if !keep_unit(record, Some(segment)) {
                continue;
            }
            docs.push(Doc {
                record: key.clone(),
                tokens: tokens(&format!("{} {}", segment.heading, segment.text)),
                weight: 1.0,
                order: 1,
                at: segment.to,
                hidden: !record.active,
                item: segment_item(record, segment),
                origin: Origin::Segment(i, j),
            });
        }
    }
    docs
}

/// Rank `docs` for `query` (§6.2 steps 0 and 3): narrow to a project or
/// topic the `words` name, score with BM25 times each unit's weight, keep
/// what scores above zero, best first (entries before sessions before
/// tasks on a tie, then newest).
pub(crate) fn rank(
    words: &[String],
    corpus: Corpus<'_>,
    mut docs: Vec<Doc>,
    query: &BTreeSet<String>,
) -> Vec<(f64, Doc)> {
    narrow_docs(words, corpus, &mut docs, query);
    let scores = bm25(&docs, query);
    let mut ranked: Vec<(f64, Doc)> = scores
        .into_iter()
        .zip(docs)
        .filter(|(score, _)| *score > 0.0)
        .collect();
    ranked.sort_by(|(sa, a), (sb, b)| {
        sb.total_cmp(sa)
            .then_with(|| a.order.cmp(&b.order))
            .then_with(|| b.at.cmp(&a.at))
    });
    ranked
}

/// How many records have a hidden unit among `ranked`, each counted once.
pub(crate) fn hidden_records(ranked: &[(f64, Doc)]) -> usize {
    ranked
        .iter()
        .filter(|(_, d)| d.hidden)
        .map(|(_, d)| d.record.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

/// Narrow store records to a project or topic the question names (§6.2
/// step 0), unless that leaves nothing that matches; tasks are kept.
fn narrow_docs(
    words: &[String],
    corpus: Corpus<'_>,
    docs: &mut Vec<Doc>,
    query: &BTreeSet<String>,
) {
    let mut named_projects: BTreeSet<String> = BTreeSet::new();
    let mut named_topics: BTreeSet<String> = BTreeSet::new();
    let no_topics: &[Topic] = &[];
    let known = corpus
        .entries
        .iter()
        .map(|e| (&e.project, e.topics.as_slice()))
        .chain(
            corpus
                .sessions
                .iter()
                .map(|r| (&r.project, r.topics.as_slice())),
        )
        .chain(
            corpus
                .sessions
                .iter()
                .flat_map(|r| r.log.iter().map(move |s| (&r.project, s.topics.as_slice()))),
        )
        // A group-only message has no session record, so `record_meta`
        // resolves it to its group's project; a message with a record is
        // already covered by that record.
        .chain(
            corpus
                .messages
                .iter()
                .filter(|m| corpus.sessions.iter().all(|r| r.id != m.session))
                .filter_map(|m| m.group_project.as_ref())
                .map(|project| (project, no_topics)),
        );
    for (project, topics) in known {
        for w in words {
            if project_named(project, w) {
                named_projects.insert(project.as_str().to_owned());
            }
            if topics.iter().any(|t| t.as_str() == w) {
                named_topics.insert(w.clone());
            }
        }
    }
    if named_projects.is_empty() && named_topics.is_empty() {
        return;
    }
    let keep = |doc: &Doc| -> bool {
        if doc.order == 2 {
            return true;
        }
        let (project, topics) = record_meta(corpus, &doc.item.cite);
        project.is_some_and(|p| named_projects.contains(p))
            || topics.iter().any(|t| named_topics.contains(t.as_str()))
    };
    let narrowed: Vec<bool> = docs.iter().map(keep).collect();
    let any_match = docs
        .iter()
        .zip(&narrowed)
        .any(|(d, k)| *k && d.order != 2 && d.tokens.iter().any(|t| query.contains(t)));
    if any_match {
        let mut keep_iter = narrowed.into_iter();
        docs.retain(|_| keep_iter.next().unwrap_or(true));
    }
}

/// The project and topics of the record a cite names. A segment carries the
/// record's topics and its own.
fn record_meta<'a>(corpus: Corpus<'a>, cite: &str) -> (Option<&'a str>, Vec<&'a Topic>) {
    if let Some(rest) = cite.strip_prefix("session:") {
        let (id, segment) = match rest.split_once('#') {
            Some((id, n)) => (id, n.parse::<u32>().ok()),
            None => (rest, None),
        };
        if let Some(r) = corpus.sessions.iter().find(|r| r.id.as_str() == id) {
            let mut topics: Vec<&Topic> = r.topics.as_slice().iter().collect();
            if let Some(s) = segment.and_then(|n| r.segment(n)) {
                topics.extend(s.topics.as_slice());
            }
            return (Some(r.project.as_str()), topics);
        }
        // A group-only message has no catalog record: its group's project
        // is all it carries (the same one `retrieve` scopes it by).
        if let Some(project) = corpus
            .messages
            .iter()
            .find(|m| m.session.as_str() == id)
            .and_then(|m| m.group_project.as_ref())
        {
            return (Some(project.as_str()), Vec::new());
        }
    } else if let Some(e) = corpus.entries.iter().find(|e| e.id.to_string() == cite) {
        return (
            Some(e.project.as_str()),
            e.topics.as_slice().iter().collect(),
        );
    }
    (None, Vec::new())
}

/// BM25 scores of `docs` for the `query` tokens, each times its weight.
fn bm25(docs: &[Doc], query: &BTreeSet<String>) -> Vec<f64> {
    let n = docs.len();
    if n == 0 {
        return Vec::new();
    }
    let lengths: Vec<f64> = docs.iter().map(|d| count_f64(d.tokens.len())).collect();
    let average = lengths.iter().sum::<f64>() / count_f64(n);
    let mut containing: BTreeMap<&str, usize> = BTreeMap::new();
    for doc in docs {
        let unique: BTreeSet<&str> = doc.tokens.iter().map(String::as_str).collect();
        for token in unique {
            if query.contains(token) {
                *containing.entry(token).or_default() += 1;
            }
        }
    }
    docs.iter()
        .zip(&lengths)
        .map(|(doc, &len)| {
            let mut score = 0.0;
            for token in query {
                let tf = count_f64(doc.tokens.iter().filter(|t| *t == token).count());
                if tf == 0.0 {
                    continue;
                }
                let df = count_f64(containing.get(token.as_str()).copied().unwrap_or(0));
                let idf = ((count_f64(n) - df + 0.5) / (df + 0.5)).ln_1p();
                let norm = if average > 0.0 { len / average } else { 1.0 };
                score +=
                    idf * tf * (BM25_K1 + 1.0) / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * norm));
            }
            score * doc.weight
        })
        .collect()
}

/// A count as a float; counts here are far below 2^52.
#[allow(clippy::cast_precision_loss)]
fn count_f64(n: usize) -> f64 {
    n as f64
}

fn entry_item(entry: &Entry, user: &UserName) -> EvidenceItem {
    let whose = match &entry.content_from {
        ContentFrom::Agent => String::from("agent"),
        ContentFrom::Instruction
        | ContentFrom::Message {
            role: Speaker::User,
            ..
        } => user.title().to_owned(),
        ContentFrom::Message {
            role: Speaker::Assistant,
            ..
        } => format!("assistant (recorded by {})", user.noun()),
    };
    // A finding is that agent's claim as of its date, with the evidence
    // pointer it can be checked against — never the user's words. The
    // pointer rides in `whose` (which the deterministic check copies
    // into each claim) and in the text a claim may quote.
    let (whose, text) = if entry.author.kind == crate::AuthorKind::Agent && entry.is_finding() {
        let evidence = entry.evidence.as_deref().unwrap_or_default();
        (
            format!(
                "agent claim ({}) as of {}; evidence: {}",
                entry.author.name,
                entry
                    .as_of
                    .map_or_else(|| day(entry.created), crate::record::DateRange::day),
                evidence
            ),
            format!("{}\nEvidence: {}", entry.content, evidence),
        )
    } else if let (Some(quote), Some(outcome)) = (stated_quote(entry), &entry.outcome) {
        // An entry the pass captured: scribe's statement, the turn's own
        // words, and where it stands (2_design.md §11.6).
        (
            captured_whose(user, speaker_of(entry)),
            format!(
                "{}\n{} \"{quote}\"\nOutcome: {}",
                entry.content,
                words_label(user, speaker_of(entry)),
                outcome_text(outcome, user)
            ),
        )
    } else if let Some(quote) = stated_quote(entry) {
        // Scribe's or an agent's statement of the turn, then the speaker's
        // own words, labelled as theirs (2_design.md §11.1, §15.3, §16.6).
        let speaker = speaker_of(entry);
        (
            stated_whose_of(user, entry),
            format!(
                "{}\n{} \"{quote}\"",
                entry.content,
                words_label(user, speaker)
            ),
        )
    } else if entry.author.kind == crate::AuthorKind::Agent {
        (
            format!(
                "agent note ({}){}",
                entry.author.name,
                entry.handoff_mode.map_or(String::new(), |m| format!(
                    " [handoff mode: {}]",
                    m.as_str()
                ))
            ),
            entry.content.clone(),
        )
    } else {
        (whose, entry.content.clone())
    };
    EvidenceItem {
        cite: entry.id.to_string(),
        kind: "entry",
        whose,
        status: entry.status.as_str().to_owned(),
        date: day(entry.created),
        text,
        quote: stated_quote(entry).map(str::to_owned),
        approval: approved_line(entry, &instruction_theirs(user)),
    }
}

/// A session's header as evidence, cited `session:<id>`.
fn session_item(record: &SessionRecord, user: &UserName) -> EvidenceItem {
    let mut text = format!(
        "{}\n## Summary\n{}\n## Goals\n{}\n## Next\n{}",
        record.title, record.summary, record.goals, record.next
    );
    for term in &record.terms {
        text.push_str(&format!(
            "\nterm \"{}\", defined by {}: \"{}\"",
            term.term,
            user.noun(),
            term.said
        ));
    }
    EvidenceItem {
        cite: format!("session:{}", record.id),
        kind: "session",
        whose: "session summary".to_owned(),
        status: record.status.as_str().to_owned(),
        date: day(record.updated),
        text,
        quote: None,
        approval: None,
    }
}

/// One segment of a session's log as evidence, cited `session:<id>#<n>`:
/// a claim citing it is checked against its heading and text alone.
fn segment_item(record: &SessionRecord, segment: &Segment) -> EvidenceItem {
    EvidenceItem {
        cite: format!("session:{}#{}", record.id, segment.n),
        kind: "session",
        whose: "session summary".to_owned(),
        status: record.status.as_str().to_owned(),
        date: day(segment.to),
        text: format!("{}\n{}", segment.heading, segment.text),
        quote: None,
        approval: None,
    }
}

/// One ranked unit's text and labels: what a hit's cite, snippet and
/// attribution are read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceItem {
    /// Its cite: an entry id, `session:<id>` or `session:<id>#<n>`.
    pub cite: String,
    /// `entry` or `session`.
    pub kind: &'static str,
    /// Whose words the text is: the user, `assistant (recorded by the user)`,
    /// `session summary` or `Orbit task`.
    pub whose: String,
    /// The record's status (`active`, `superseded`, `open`, `done`, ...).
    pub status: String,
    /// When it was recorded or last changed, `YYYY-MM-DD`.
    pub date: String,
    /// The text a claim may quote.
    pub text: String,
    /// The user's verbatim words within `text`, for an entry whose content is
    /// scribe's statement of their turn (2_design.md §11.1): a claim is his
    /// only when its quote lies within these. `None` for every other item.
    pub quote: Option<String>,
    /// For an entry an agent approved on the user's checked instruction, the
    /// approved-by line (2_design.md §12.3); `None` otherwise. It sits
    /// outside `text`, so no claim can quote it as the entry's words.
    pub approval: Option<String>,
}
