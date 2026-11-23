//! `ask`: answer a question from the store, with verbatim-checked quotes
//! (docs/design/scribe/2_design.md §6).
//!
//! [`answer`] runs the whole operation over records the caller loaded and
//! two ports it supplies, so the CLI's `question ask` and the Orbit tool
//! `scribe.ask` share one implementation and return the same payload:
//!
//! 1. **Lookups** ([`Lookup`], §6.5), model-free: the newest session, a
//!    named session, open next steps, a tag's entries in a period, a term
//!    the user defined, and the newest orchestration task for a workspace.
//!    A lookup returns the stored records themselves (`status: record`)
//!    without a model call; one that finds nothing falls through.
//! 2. **Retrieval** (§6.2, §6.7), model-free: scope filters, tokenizing,
//!    BM25 over entries, session headers and session segments (separate
//!    units, a segment cited `session:<id>#<n>`) and up to [`MAX_TASKS`]
//!    Orbit tasks, the top
//!    [`TOP_K`] packed newest-first within [`EVIDENCE_CHARS`]. Nothing
//!    scoring above zero is `no_record` with no model call.
//! 3. **Answering** (§6.3, §6.4): the `ask` model through the [`Models`]
//!    port; every claim whose cite is not in the evidence, or whose quote is
//!    not verbatim in that item, is dropped and counted.
//!
//! Inactive sessions, like superseded and retired entries, are left out of
//! lookups and evidence unless `include_hidden`, and retrieval counts the
//! ones that matched in `hidden_matches`. Entries under an action tag
//! (`standard`, `task`, `friction`, …) are work items, not memory: they are
//! left out unless the scope names their tag ([`without_actions`]).
//!
//! Orbit tasks arrive through the [`Tasks`] port, which the CLI crate
//! implements over `orbit tool run` (`OrbitTasks`). A task source that
//! fails costs only its evidence: `ask` still answers from the store.
//!
//! No I/O (STD-02 R5).

use crate::entry::{ContentFrom, Entry, EntryStatus, Speaker, TagName};
use crate::models::{ASK_BUDGET, AskAnswer, AskRequest, EvidenceItem, ModelError, Models};
use crate::record::{Project, Topic};
use crate::session::{Segment, SessionId, SessionRecord, Term};
use crate::tag::TagTable;
use crate::verbatim;
use crate::voice::{UserName, Voice};
use std::collections::{BTreeMap, BTreeSet};
use time::{Date, Duration, OffsetDateTime};

/// The most Orbit tasks retrieval reads (§6.2).
pub const MAX_TASKS: usize = 20;
/// The most evidence items the model receives.
pub const TOP_K: usize = 20;
/// The evidence budget, in characters of item text.
pub const EVIDENCE_CHARS: usize = 48_000;
/// How much of a task's description is evidence.
pub const TASK_DESCRIPTION_CHARS: usize = 1_000;
/// The longest question accepted, in characters.
pub const MAX_QUESTION_CHARS: usize = 2_000;
/// The most records a lookup returns.
pub const MAX_RECORDS: usize = 10;
/// Entries are the user's explicit words, so they outrank summaries (§6.2).
const ENTRY_WEIGHT: f64 = 1.5;
/// An entry in a tag the question names ranks higher again.
const TAG_BOOST: f64 = 1.5;
/// BM25's term-frequency saturation.
const BM25_K1: f64 = 1.2;
/// BM25's length normalization.
const BM25_B: f64 = 0.75;
/// The tag the orchestrator lookup searches for (§6.5).
pub const ORCHESTRATION_TAG: &str = "orchestration";

/// Why a question was refused before anything ran.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuestionError {
    /// The question was empty or only whitespace.
    #[error("the question is empty")]
    Empty,
    /// The question was longer than [`MAX_QUESTION_CHARS`].
    #[error("the question is longer than {MAX_QUESTION_CHARS} characters")]
    TooLong,
}

/// Check a question's text: not blank, at most [`MAX_QUESTION_CHARS`].
pub fn check_question(text: &str) -> Result<(), QuestionError> {
    if text.trim().is_empty() {
        Err(QuestionError::Empty)
    } else if text.chars().count() > MAX_QUESTION_CHARS {
        Err(QuestionError::TooLong)
    } else {
        Ok(())
    }
}

/// Which record kind a scope keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordKind {
    /// Entries only.
    Entry,
    /// Session records only.
    Session,
    /// Stored messages only (`scribe.db`, 2_design.md §15.7). `ask` does not
    /// retrieve them yet, so a scope of this kind holds no entry or session.
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

/// What the caller restricts the candidates to (§6.1). Every field set must
/// match. Tags and topics are entry and session notions, so a scope with
/// tags, topics or a kind leaves Orbit tasks out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    /// Only entries in one of these tags (sessions have no tag, so they drop).
    pub tags: Vec<TagName>,
    /// Only records carrying one of these topics.
    pub topics: Vec<Topic>,
    /// Only records of this project.
    pub project: Option<Project>,
    /// Only this kind of record.
    pub kind: Option<RecordKind>,
    /// Only records changed on or after this day.
    pub since: Option<Date>,
}

impl Scope {
    fn keeps_entry(&self, entry: &Entry) -> bool {
        self.kind.is_none_or(|k| k == RecordKind::Entry)
            && (self.tags.is_empty() || self.tags.contains(&entry.tag))
            && (self.topics.is_empty() || self.topics.iter().any(|t| entry.topics.contains(t)))
            && self.project.as_ref().is_none_or(|p| &entry.project == p)
            && self.since.is_none_or(|d| entry.updated.date() >= d)
    }

    fn keeps_session(&self, record: &SessionRecord) -> bool {
        self.kind.is_none_or(|k| k == RecordKind::Session)
            && self.tags.is_empty()
            && (self.topics.is_empty() || self.topics.iter().any(|t| record.topics.contains(t)))
            && self.project.as_ref().is_none_or(|p| &record.project == p)
            && self.since.is_none_or(|d| record.updated.date() >= d)
    }

    fn keeps_tasks(&self) -> bool {
        self.kind.is_none() && self.tags.is_empty() && self.topics.is_empty()
    }

    fn keeps_task(&self, task: &Task) -> bool {
        self.since
            .is_none_or(|d| task.updated.is_some_and(|u| u.date() >= d))
    }
}

/// A question and how to read the store for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    /// The question as asked.
    pub text: String,
    /// The candidate restriction.
    pub scope: Scope,
    /// Whether superseded and retired entries and inactive sessions are
    /// evidence too.
    pub include_hidden: bool,
}

/// One Orbit task, as the [`Tasks`] port returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    /// The task id.
    pub id: String,
    /// Its title.
    pub title: String,
    /// Its status (`done`, `in-progress`, ...).
    pub status: String,
    /// Its tags.
    pub tags: Vec<String>,
    /// When it last changed, when known.
    pub updated: Option<OffsetDateTime>,
    /// Its description, whole.
    pub description: String,
    /// Its execution summary, verbatim; empty when it has none.
    pub execution_summary: String,
}

/// What to ask the task source for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskQuery {
    /// The search words.
    pub text: String,
    /// Tags every task must carry.
    pub tags: Vec<String>,
    /// The most tasks to return.
    pub limit: usize,
}

/// The task source failed; `ask` goes on without task evidence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Orbit tasks are unavailable: {0}")]
pub struct TaskError(pub String);

/// Where Orbit tasks come from. The CLI crate implements it over
/// `orbit tool run`; tests implement it with fixtures.
pub trait Tasks {
    /// Tasks matching `query`, each in full (description and execution
    /// summary included).
    ///
    /// # Errors
    ///
    /// A [`TaskError`] when the source cannot be reached or answers in
    /// another shape.
    fn search(&self, query: &TaskQuery) -> Result<Vec<Task>, TaskError>;
}

/// A task source with nothing in it.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTasks;

impl Tasks for NoTasks {
    fn search(&self, _query: &TaskQuery) -> Result<Vec<Task>, TaskError> {
        Ok(Vec::new())
    }
}

/// The records `ask` reads, loaded by the caller.
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

/// How an answer stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The evidence answers it; every claim survived the check.
    Answered,
    /// Nothing recorded answers it.
    NoRecord,
    /// Two records disagree; both are cited.
    Conflict,
    /// A lookup: the stored records themselves.
    Record,
}

impl Status {
    /// The canonical token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::NoRecord => "no_record",
            Self::Conflict => "conflict",
            Self::Record => "record",
        }
    }
}

/// Which lookup answered (§6.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The newest session record in scope.
    LastSession,
    /// A session named by id or title.
    NamedSession,
    /// The open next steps of the newest records.
    NextSteps,
    /// A tag's entries in a period.
    TagPeriod,
    /// The records where the user defined a term.
    Term,
    /// The newest orchestration task naming a workspace.
    OrchestratorSession,
}

impl Lookup {
    /// The canonical token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LastSession => "last_session",
            Self::NamedSession => "named_session",
            Self::NextSteps => "next_steps",
            Self::TagPeriod => "tag_period",
            Self::Term => "term",
            Self::OrchestratorSession => "orchestrator_session",
        }
    }
}

/// A record a lookup returns, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// An entry (boxed: an entry is much larger than a task).
    Entry(Box<Entry>),
    /// A session record, with the terms that matched (empty unless the
    /// lookup was for a term).
    Session {
        /// The record (boxed, as an entry is, to keep the enum small).
        record: Box<SessionRecord>,
        /// The matching terms and their defining spans.
        terms: Vec<Term>,
    },
    /// An Orbit task.
    Task(Task),
}

impl Found {
    /// How an answer cites it.
    #[must_use]
    pub fn cite(&self) -> String {
        match self {
            Self::Entry(entry) => entry.id.to_string(),
            Self::Session { record, .. } => format!("session:{}", record.id),
            Self::Task(task) => format!("task:{}", task.id),
        }
    }
}

/// A claim that survived the verbatim check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    /// The statement.
    pub text: String,
    /// Words verbatim from the cited item.
    pub quote: String,
    /// The item: an entry id, `session:<id>`, `session:<id>#<n>` (a
    /// segment) or `task:<id>`.
    pub cite: String,
}

/// A disagreement between cited items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The items that disagree, each in the evidence.
    pub cites: Vec<String>,
    /// What they disagree about.
    pub note: String,
}

/// What `ask` returns (§6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// How it stands.
    pub status: Status,
    /// The answer text.
    pub answer: String,
    /// The claims that survived.
    pub claims: Vec<Claim>,
    /// The disagreements, each between items in the evidence.
    pub conflicts: Vec<Conflict>,
    /// How many items the model was given (records returned, for a lookup).
    pub evidence_considered: usize,
    /// Claims dropped because their quote or cite failed.
    pub dropped_quotes: usize,
    /// Hidden entries and inactive sessions that matched but were left out
    /// (`include_hidden` off); a session counts once however many of its
    /// units matched.
    pub hidden_matches: usize,
    /// Which lookup answered, if one did.
    pub lookup: Option<Lookup>,
    /// The stored records a lookup returned; empty otherwise.
    pub records: Vec<Found>,
}

/// The answer text when nothing recorded answers.
pub const NO_RECORD: &str = "Nothing recorded answers this.";
/// Appended when a hidden entry or an inactive session matched and was
/// left out.
pub const HIDDEN_NOTE: &str = "A superseded or retired entry, or an inactive session, also matches; ask with include_hidden to see it.";

/// Answer `question` from `corpus`, the tasks `tasks` finds and the `ask`
/// model `model` (an id from `config.toml`) voiced with `voice`, at time
/// `now`.
///
/// # Errors
///
/// The [`ModelError`] of a failed model call. A failed task source is not
/// an error: its evidence is left out.
pub fn answer<M: Models + ?Sized, T: Tasks + ?Sized>(
    question: &Question,
    corpus: Corpus<'_>,
    models: &M,
    tasks: &T,
    model: &str,
    voice: &Voice,
    now: OffsetDateTime,
) -> Result<Answer, ModelError> {
    // Action entries are work items, not memory: they are evidence only
    // when the question's scope names their tag (§4.6).
    let kept = without_actions(question, corpus);
    let corpus = Corpus {
        entries: &kept,
        ..corpus
    };
    if let Some(found) = lookup(question, corpus, tasks, now) {
        return Ok(found);
    }
    let retrieved = retrieve(question, corpus, tasks);
    if retrieved.evidence.is_empty() {
        return Ok(no_record(0, 0, retrieved.hidden_matches));
    }
    let request = AskRequest {
        model: model.to_owned(),
        budget: ASK_BUDGET,
        question: question.text.clone(),
        evidence: retrieved.evidence,
        voice: voice.clone(),
    };
    let reply = models.ask(&request)?;
    Ok(check(
        reply,
        &request.evidence,
        retrieved.hidden_matches,
        corpus.user,
    ))
}

/// `corpus`'s entries less those under an action tag the question's scope
/// does not name.
#[must_use]
pub fn without_actions(question: &Question, corpus: Corpus<'_>) -> Vec<Entry> {
    corpus
        .entries
        .iter()
        .filter(|e| corpus.tags.action(&e.tag).is_none() || question.scope.tags.contains(&e.tag))
        .cloned()
        .collect()
}

fn no_record(considered: usize, dropped: usize, hidden: usize) -> Answer {
    Answer {
        status: Status::NoRecord,
        answer: with_hidden_note(NO_RECORD.to_owned(), hidden),
        claims: Vec::new(),
        conflicts: Vec::new(),
        evidence_considered: considered,
        dropped_quotes: dropped,
        hidden_matches: hidden,
        lookup: None,
        records: Vec::new(),
    }
}

fn with_hidden_note(text: String, hidden: usize) -> String {
    if hidden == 0 {
        text
    } else if text.is_empty() {
        HIDDEN_NOTE.to_owned()
    } else {
        format!("{text} {HIDDEN_NOTE}")
    }
}

/// Check the model's answer against the evidence it was given (§6.4): a
/// claim survives only if its cite names an item and its quote is verbatim
/// in that item's text; a conflict survives only if it names at least two
/// items. With no claim and no conflict left, the answer is `no_record`.
/// `user` names the user in the labels a claim gets.
#[must_use]
pub fn check(
    reply: AskAnswer,
    evidence: &[EvidenceItem],
    hidden: usize,
    user: &UserName,
) -> Answer {
    let text_of = |cite: &str| evidence.iter().find(|e| e.cite == cite).map(|e| &e.text);
    let offered = reply.claims.len();
    let claims: Vec<Claim> = reply
        .claims
        .into_iter()
        .filter(|c| text_of(&c.cite).is_some_and(|text| verbatim::contains(text, &c.quote)))
        .map(|c| Claim {
            text: c.text,
            quote: c.quote,
            cite: c.cite,
        })
        .collect();
    let dropped = offered - claims.len();
    let conflicts: Vec<Conflict> = reply
        .conflicts
        .into_iter()
        .filter_map(|c| {
            let mut cites: Vec<String> = Vec::new();
            for cite in c.cites {
                if text_of(&cite).is_some() && !cites.contains(&cite) {
                    cites.push(cite);
                }
            }
            (cites.len() >= 2).then_some(Conflict {
                cites,
                note: c.note,
            })
        })
        .collect();
    let status = if !conflicts.is_empty() {
        Status::Conflict
    } else if reply.status == "no_record" || claims.is_empty() {
        Status::NoRecord
    } else {
        Status::Answered
    };
    if status == Status::NoRecord {
        return no_record(evidence.len(), dropped, hidden);
    }
    // Agent notes and scribe's statements are never the user's words, so
    // with either in the evidence every claim is labelled by whose words
    // its quote is, deterministically, and the answer is built from those
    // labels rather than the model's prose.
    let has_agent = evidence.iter().any(|e| {
        e.whose.starts_with("agent note (")
            || e.whose.starts_with("agent claim (")
            || e.whose.starts_with("agent account (")
            || e.quote.is_some()
    });
    let answer = if has_agent {
        claims
            .iter()
            .map(|c| {
                format!(
                    "{}: \"{}\" [{}]",
                    attribution(c, evidence, user),
                    c.quote,
                    c.cite
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        reply.answer
    };
    let claims = if has_agent {
        claims
            .into_iter()
            .map(|mut c| {
                c.text = attribution(&c, evidence, user);
                c
            })
            .collect()
    } else {
        claims
    };
    let conflicts: Vec<Conflict> = if has_agent {
        conflicts
            .into_iter()
            .map(|mut c| {
                c.note = format!(
                    "The cited sources disagree: {}.",
                    c.cites
                        .iter()
                        .filter_map(|cite| evidence.iter().find(|e| &e.cite == cite))
                        .map(|e| format!("{} [{}]", e.whose, e.cite))
                        .collect::<Vec<_>>()
                        .join("; ")
                );
                c
            })
            .collect()
    } else {
        conflicts
    };
    let answer = if has_agent && answer.is_empty() {
        conflicts
            .iter()
            .map(|c| c.note.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        answer
    };
    Answer {
        status,
        answer: with_hidden_note(answer, hidden),
        claims,
        conflicts,
        evidence_considered: evidence.len(),
        dropped_quotes: dropped,
        hidden_matches: hidden,
        lookup: None,
        records: Vec::new(),
    }
}

/// Whose words a checked claim's quote is (§6.4, §11.1, §15.3): the cited
/// item's `whose`, except that a quote from an entry holding scribe's
/// statement of a turn is that speaker's words only when it lies within
/// `quote`; anywhere else in that item it is scribe's statement.
fn attribution(claim: &Claim, evidence: &[EvidenceItem], user: &UserName) -> String {
    match evidence.iter().find(|e| e.cite == claim.cite) {
        None => String::from("record"),
        Some(EvidenceItem {
            quote: Some(theirs),
            whose,
            ..
        }) if verbatim::contains(theirs, &claim.quote) => {
            if *whose == captured_whose(user, Speaker::User) {
                captured_user(user, Speaker::User)
            } else if *whose == captured_whose(user, Speaker::Assistant) {
                captured_user(user, Speaker::Assistant)
            } else if *whose == stated_whose_for(user, Speaker::Assistant) {
                format!("assistant (recorded by {})", user.noun())
            } else {
                user.title().to_owned()
            }
        }
        Some(item) => item.whose.clone(),
    }
}

// --- words -------------------------------------------------------------------

/// Words that carry no meaning for ranking.
const STOPWORDS: &[&str] = &[
    "about", "above", "after", "again", "all", "also", "and", "any", "are", "because", "been",
    "before", "being", "between", "both", "but", "can", "could", "did", "does", "doing", "done",
    "each", "for", "from", "had", "has", "have", "having", "her", "here", "him", "his", "how",
    "into", "its", "just", "let", "like", "made", "make", "may", "more", "most", "much", "must",
    "not", "now", "off", "once", "only", "other", "our", "out", "over", "own", "please", "same",
    "say", "said", "she", "should", "some", "such", "than", "that", "the", "their", "them", "then",
    "there", "these", "they", "this", "those", "through", "too", "under", "until", "very", "was",
    "way", "were", "what", "when", "where", "which", "while", "who", "whom", "why", "will", "with",
    "would", "yes", "you", "your", "yours",
];

/// The ranking tokens of `text` (§6.2): lowercase, split on anything not a
/// letter or digit, stopwords and tokens under three characters dropped,
/// and a light suffix strip (`-ing`, `-ed`, `-es`, `-s`, then a final `-e`,
/// so `merge`, `merges`, `merged` and `merging` are one token).
#[must_use]
pub fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && !STOPWORDS.contains(w))
        .map(stem)
        .collect()
}

fn stem(word: &str) -> String {
    let mut stem = word;
    for suffix in ["ing", "ed", "es", "s"] {
        if let Some(base) = stem.strip_suffix(suffix)
            && base.chars().count() >= 3
            && !(suffix == "s" && base.ends_with('s'))
        {
            stem = base;
            break;
        }
    }
    if stem.chars().count() > 3
        && let Some(base) = stem.strip_suffix('e')
    {
        stem = base;
    }
    stem.to_owned()
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

/// `text` lowercased with every run of whitespace as one space, for phrase
/// matches.
fn folded(text: &str) -> String {
    verbatim::normalize(text).trim().to_lowercase()
}

/// Whether `phrase` occurs in `text` (both folded) on word boundaries.
fn has_phrase(text: &str, phrase: &str) -> bool {
    if phrase.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(at) = text.get(from..).and_then(|rest| rest.find(phrase)) {
        let start = from + at;
        let end = start + phrase.len();
        let before = text.get(..start).and_then(|t| t.chars().next_back());
        let after = text.get(end..).and_then(|t| t.chars().next());
        if before.is_none_or(|c| !c.is_alphanumeric()) && after.is_none_or(|c| !c.is_alphanumeric())
        {
            return true;
        }
        from = start + phrase.chars().next().map_or(1, char::len_utf8);
    }
    false
}

const RECENT: &[&str] = &["last", "latest", "previous", "recent", "newest"];

fn any_word(words: &[String], of: &[&str]) -> bool {
    words.iter().any(|w| of.contains(&w.as_str()))
}

// --- lookups (§6.5) ----------------------------------------------------------

/// Run the lookup classifier: the stored records a lookup question asks
/// for, or `None` when the question is not a lookup or the lookup found
/// nothing (both fall through to retrieval).
fn lookup<T: Tasks + ?Sized>(
    question: &Question,
    corpus: Corpus<'_>,
    tasks: &T,
    now: OffsetDateTime,
) -> Option<Answer> {
    let words = words(&question.text);
    let scope = &question.scope;
    let sessions: Vec<&SessionRecord> = newest_sessions(
        corpus
            .sessions
            .iter()
            .filter(|r| scope.keeps_session(r) && (question.include_hidden || r.active))
            .collect(),
    );
    let entries: Vec<&Entry> = corpus
        .entries
        .iter()
        .filter(|e| scope.keeps_entry(e) && visible(e, question.include_hidden))
        .collect();
    let sessionish = any_word(&words, &["session", "sessions"]);
    let recent = any_word(&words, RECENT);

    if let Some(found) = named_session(&question.text, &sessions) {
        return Some(record_answer(
            Lookup::NamedSession,
            found,
            "The named session record, as stored.",
        ));
    }
    if sessionish && recent && is_orchestration(&words) {
        let workspace = workspace_named(&words)?;
        if scope.keeps_tasks()
            && let Some(task) = orchestrator_task(tasks, &workspace)
        {
            let text = format!(
                "The newest {ORCHESTRATION_TAG} task naming {workspace}, with its execution summary verbatim."
            );
            return Some(record_answer(
                Lookup::OrchestratorSession,
                vec![Found::Task(task)],
                &text,
            ));
        }
        return None;
    }
    if sessionish && recent {
        let narrowed = narrow_sessions(&words, &sessions);
        if let Some(newest) = narrowed.first() {
            return Some(record_answer(
                Lookup::LastSession,
                vec![session_found(newest)],
                "The newest session record in scope, as stored.",
            ));
        }
        return None;
    }
    if is_next_steps(&words) {
        let open: Vec<Found> = narrow_sessions(&words, &sessions)
            .into_iter()
            .filter(|r| r.next_open > 0)
            .take(MAX_RECORDS)
            .map(session_found)
            .collect();
        if !open.is_empty() {
            return Some(record_answer(
                Lookup::NextSteps,
                open,
                "The open next steps of the newest session records, newest first, as stored.",
            ));
        }
        return None;
    }
    if let Some(found) = tag_period(&words, corpus.tags, &entries, now) {
        return Some(found);
    }
    term_lookup(&question.text, &sessions, &entries, corpus.user)
}

fn visible(entry: &Entry, include_hidden: bool) -> bool {
    match entry.status {
        EntryStatus::Active => true,
        EntryStatus::Superseded | EntryStatus::Retired => include_hidden,
        EntryStatus::Forgotten => false,
    }
}

/// Newest `updated` first, then id.
fn newest_sessions(mut sessions: Vec<&SessionRecord>) -> Vec<&SessionRecord> {
    sessions.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    sessions
}

fn session_found(record: &SessionRecord) -> Found {
    Found::Session {
        record: Box::new(record.clone()),
        terms: Vec::new(),
    }
}

fn record_answer(lookup: Lookup, records: Vec<Found>, text: &str) -> Answer {
    Answer {
        status: Status::Record,
        answer: text.to_owned(),
        claims: Vec::new(),
        conflicts: Vec::new(),
        evidence_considered: records.len(),
        dropped_quotes: 0,
        hidden_matches: 0,
        lookup: Some(lookup),
        records,
    }
}

/// A session named by `session:<id>`, or one whose whole title the question
/// contains.
fn named_session(text: &str, sessions: &[&SessionRecord]) -> Option<Vec<Found>> {
    for token in text.split_whitespace() {
        let Some(id) = token.strip_prefix("session:") else {
            continue;
        };
        // A segment cite names its session: the lookup returns the record.
        let id = id.split_once('#').map_or(id, |(id, _)| id);
        let id = id.trim_end_matches(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'));
        let Ok(id) = id.parse::<SessionId>() else {
            continue;
        };
        if let Some(record) = sessions.iter().find(|r| r.id == id) {
            return Some(vec![session_found(record)]);
        }
    }
    let question = folded(text);
    sessions
        .iter()
        .find(|r| {
            let title = folded(&r.title);
            title.chars().count() >= verbatim::MIN_QUOTE_CHARS && has_phrase(&question, &title)
        })
        .map(|r| vec![session_found(r)])
}

fn is_orchestration(words: &[String]) -> bool {
    any_word(
        words,
        &[
            "orchestrator",
            "orchestration",
            "on-call",
            "oncall",
            "orchestrators",
        ],
    )
}

/// The workspace a question names: the word after its last `for`, `in` or
/// `of`, skipping `the` and `workspace`.
fn workspace_named(words: &[String]) -> Option<String> {
    let at = words
        .iter()
        .rposition(|w| matches!(w.as_str(), "for" | "in" | "of"))?;
    words
        .iter()
        .skip(at + 1)
        .find(|w| !matches!(w.as_str(), "the" | "workspace" | "ws"))
        .cloned()
}

/// The newest `orchestration` task whose title names `workspace` as a word:
/// the newest with an execution summary, else the newest.
fn orchestrator_task<T: Tasks + ?Sized>(tasks: &T, workspace: &str) -> Option<Task> {
    let query = TaskQuery {
        text: workspace.to_owned(),
        tags: vec![ORCHESTRATION_TAG.to_owned()],
        limit: MAX_TASKS,
    };
    let found = match tasks.search(&query) {
        Ok(found) => found,
        Err(err) => {
            tracing::warn!(%err, "the orchestrator lookup could not read Orbit tasks");
            return None;
        }
    };
    let mut matching: Vec<Task> = found
        .into_iter()
        .filter(|t| t.tags.iter().any(|tag| tag == ORCHESTRATION_TAG))
        .filter(|t| words(&t.title).iter().any(|w| w == workspace))
        .collect();
    matching.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    let summarized = matching
        .iter()
        .position(|t| !t.execution_summary.trim().is_empty());
    match summarized {
        Some(at) => Some(matching.swap_remove(at)),
        None => matching.into_iter().next(),
    }
}

/// `sessions` narrowed to a project or topic the question names, when it
/// names one; otherwise unchanged. Order is kept.
fn narrow_sessions<'a>(words: &[String], sessions: &[&'a SessionRecord]) -> Vec<&'a SessionRecord> {
    let names_project = |r: &SessionRecord| words.iter().any(|w| project_named(&r.project, w));
    let names_topic = |r: &SessionRecord| {
        r.topics
            .as_slice()
            .iter()
            .any(|t| words.iter().any(|w| w == t.as_str()))
    };
    let projects: Vec<&SessionRecord> = sessions
        .iter()
        .copied()
        .filter(|r| names_project(r))
        .collect();
    if !projects.is_empty() {
        return projects;
    }
    let topics: Vec<&SessionRecord> = sessions
        .iter()
        .copied()
        .filter(|r| names_topic(r))
        .collect();
    if !topics.is_empty() {
        return topics;
    }
    let names_any_known = words.iter().any(|w| {
        sessions.iter().any(|r| {
            project_named(&r.project, w) || r.topics.as_slice().iter().any(|t| t.as_str() == w)
        })
    });
    if names_any_known {
        Vec::new()
    } else {
        sessions.to_vec()
    }
}

/// Whether `word` names `project`: the whole slug or its last path segment
/// (`scribe` names `constellation-works/scribe`). `global` names nothing.
fn project_named(project: &Project, word: &str) -> bool {
    let slug = project.as_str().to_lowercase();
    slug != Project::GLOBAL
        && word.chars().count() >= 3
        && (slug == word || slug.rsplit('/').next() == Some(word))
}

fn is_next_steps(words: &[String]) -> bool {
    let has = |w: &str| words.iter().any(|x| x == w);
    (has("next") && (has("step") || has("steps")))
        || has("pending")
        || (has("open") && (has("items") || has("todos")))
}

/// "preferences this week": entries of a named tag since the period began.
fn tag_period(
    words: &[String],
    tags: &TagTable,
    entries: &[&Entry],
    now: OffsetDateTime,
) -> Option<Answer> {
    let tag = words.iter().find_map(|w| tags.resolve(w))?;
    let today = now.date();
    let has = |w: &str| words.iter().any(|x| x == w);
    let since = if has("today") {
        today
    } else if has("this") && has("week") {
        today - Duration::days(i64::from(today.weekday().number_days_from_monday()))
    } else if has("this") && has("month") {
        today.replace_day(1).ok()?
    } else if has("this") && has("year") {
        today.replace_ordinal(1).ok()?
    } else {
        return None;
    };
    let mut found: Vec<&Entry> = entries
        .iter()
        .copied()
        .filter(|e| e.tag == tag.name && e.created.date() >= since)
        .collect();
    if found.is_empty() {
        return None;
    }
    found.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.id.cmp(&b.id)));
    let text = format!(
        "The entries in {} since {since}, newest first, as stored.",
        tag.name
    );
    Some(record_answer(
        Lookup::TagPeriod,
        found
            .into_iter()
            .take(MAX_RECORDS)
            .map(|e| Found::Entry(Box::new(e.clone())))
            .collect(),
        &text,
    ))
}

/// Leading phrases that ask what a term means.
const TERM_PREFIXES: &[&str] = &[
    "what did i mean by",
    "what do i mean by",
    "what i meant by",
    "what is meant by",
    "the meaning of",
    "meaning of",
    "what does",
    "what is",
    "what's",
    "define",
];

/// The phrase a question asks about, when it asks only about a phrase:
/// "graduate to level 4", "what did I mean by resident orchestrator?".
fn asked_phrase(text: &str) -> String {
    let mut phrase = folded(text);
    phrase = phrase
        .trim_matches(|c: char| c.is_whitespace() || "?.!\"'`“”‘’".contains(c))
        .to_owned();
    for prefix in TERM_PREFIXES {
        if let Some(rest) = phrase.strip_prefix(prefix)
            && rest.starts_with(' ')
        {
            phrase = rest.trim().to_owned();
            break;
        }
    }
    for suffix in [" mean", " means"] {
        if let Some(rest) = phrase.strip_suffix(suffix) {
            phrase = rest.trim().to_owned();
        }
    }
    phrase
        .trim_matches(|c: char| c.is_whitespace() || "?.!\"'`“”‘’".contains(c))
        .to_owned()
}

/// The session records whose `terms` hold the phrase the question asks
/// about, newest first, each with its matching terms; then the entries whose
/// body contains it.
fn term_lookup(
    text: &str,
    sessions: &[&SessionRecord],
    entries: &[&Entry],
    user: &UserName,
) -> Option<Answer> {
    let phrase = asked_phrase(text);
    if phrase.chars().count() < 3 {
        return None;
    }
    let mut found: Vec<Found> = sessions
        .iter()
        .filter_map(|r| {
            let terms: Vec<Term> = r
                .terms
                .iter()
                .filter(|t| folded(&t.term) == phrase)
                .cloned()
                .collect();
            (!terms.is_empty()).then(|| Found::Session {
                record: Box::new((*r).clone()),
                terms,
            })
        })
        .take(MAX_RECORDS)
        .collect();
    if found.is_empty() {
        return None;
    }
    let mut bodies: Vec<&Entry> = entries
        .iter()
        .copied()
        .filter(|e| has_phrase(&folded(&e.content), &phrase))
        .collect();
    bodies.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.id.cmp(&b.id)));
    let room = MAX_RECORDS.saturating_sub(found.len());
    found.extend(
        bodies
            .into_iter()
            .take(room)
            .map(|e| Found::Entry(Box::new(e.clone()))),
    );
    let text = format!(
        "The session records where {} defined \"{phrase}\", newest first, with the defining words, then the entries that use it.",
        user.noun()
    );
    Some(record_answer(Lookup::Term, found, &text))
}

// --- retrieval (§6.2) --------------------------------------------------------

/// What retrieval packed for the model.
struct Retrieved {
    evidence: Vec<EvidenceItem>,
    hidden_matches: usize,
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
    /// A stored message.
    Message,
    /// An Orbit task.
    Task,
}

/// A candidate unit, ready to rank: an entry, a session header or segment,
/// or a task.
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
/// weighted as `ask` weights them; an entry in a tag the `words` name is
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

fn retrieve<T: Tasks + ?Sized>(question: &Question, corpus: Corpus<'_>, tasks: &T) -> Retrieved {
    let query: BTreeSet<String> = tokens(&question.text).into_iter().collect();
    if query.is_empty() {
        return Retrieved {
            evidence: Vec::new(),
            hidden_matches: 0,
        };
    }
    let words = words(&question.text);
    let scope = &question.scope;
    let mut docs = store_docs(
        &words,
        corpus,
        |e| scope.keeps_entry(e),
        |r, _| scope.keeps_session(r),
    );
    if scope.kind.is_none() && scope.tags.is_empty() {
        docs.extend(corpus.messages.iter().filter_map(|m| {
            let record = corpus.sessions.iter().find(|r| r.id == m.session);
            let project = record.map(|r| &r.project).or(m.group_project.as_ref());
            if scope.project.as_ref().is_some_and(|p| Some(p) != project)
                || (!scope.topics.is_empty()
                    && !scope
                        .topics
                        .iter()
                        .any(|t| record.is_some_and(|r| r.topics.contains(t))))
                || scope.since.is_some_and(|since| m.at.date() < since)
            {
                return None;
            }
            let session = format!("session:{}", m.session);
            Some(Doc {
                item: EvidenceItem {
                    cite: format!("{session}#message:{}", m.message),
                    kind: "message",
                    whose: words_label(corpus.user, m.role),
                    status: "stored".into(),
                    date: day(m.at),
                    text: m.text.clone(),
                    quote: None,
                    approval: None,
                },
                record: session,
                tokens: tokens(&m.text),
                weight: 1.0,
                order: 1,
                at: m.at,
                hidden: record.is_some_and(|r| !r.active),
                origin: Origin::Message,
            })
        }));
    }
    if scope.keeps_tasks() {
        let query_text: Vec<&str> = query.iter().map(String::as_str).collect();
        let found = tasks.search(&TaskQuery {
            text: query_text.join(" "),
            tags: Vec::new(),
            limit: MAX_TASKS,
        });
        match found {
            Ok(found) => {
                for task in found.iter().filter(|t| scope.keeps_task(t)).take(MAX_TASKS) {
                    let description: String = task
                        .description
                        .chars()
                        .take(TASK_DESCRIPTION_CHARS)
                        .collect();
                    docs.push(Doc {
                        record: format!("task:{}", task.id),
                        tokens: tokens(&format!("{} {description}", task.title)),
                        weight: 1.0,
                        order: 2,
                        at: task.updated.unwrap_or(OffsetDateTime::UNIX_EPOCH),
                        hidden: false,
                        item: task_item(task, &description),
                        origin: Origin::Task,
                    });
                }
            }
            Err(err) => tracing::warn!(%err, "ask goes on without Orbit task evidence"),
        }
    }
    let mut ranked = rank(&words, corpus, docs, &query);
    let hidden_matches = if question.include_hidden {
        0
    } else {
        hidden_records(&ranked)
    };
    ranked.retain(|(_, d)| question.include_hidden || !d.hidden);
    ranked.truncate(TOP_K);
    ranked.sort_by(|(_, a), (_, b)| b.at.cmp(&a.at).then_with(|| a.order.cmp(&b.order)));
    let mut budget = EVIDENCE_CHARS;
    let mut evidence = Vec::new();
    for (_, doc) in ranked {
        let chars = doc.item.text.chars().count();
        if chars <= budget {
            budget -= chars;
            evidence.push(doc.item);
        }
    }
    Retrieved {
        evidence,
        hidden_matches,
    }
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

fn day(at: OffsetDateTime) -> String {
    crate::record::nullable_date::format(at.date())
}

/// Whose words an approved pass statement is, in `ask` evidence and in
/// `scribe search` snippets (2_design.md §11.1).
#[must_use]
pub fn stated_whose(user: &UserName) -> String {
    format!("scribe's statement of {}'s turn", user.noun())
}

/// Whose words an approved pass statement of `speaker`'s turn is, in
/// `ask` evidence, `scribe search` snippets and `entry show`
/// (2_design.md §11.1, §15.3).
#[must_use]
pub fn stated_whose_for(user: &UserName, speaker: Speaker) -> String {
    match speaker {
        Speaker::User => stated_whose(user),
        Speaker::Assistant => String::from("scribe's statement of the assistant's turn"),
    }
}

/// Whose words an agent's statement of the user's spoken instruction is, in
/// `ask` evidence, `scribe search` snippets and `entry show` (2_design.md
/// §16.6).
#[must_use]
pub fn agent_stated_whose(user: &UserName, agent: &str) -> String {
    format!("agent ({agent})'s statement of {}'s turn", user.noun())
}

/// Whose words `entry`'s content is when `stated_quote` returned its quote.
#[must_use]
pub fn stated_whose_of(user: &UserName, entry: &Entry) -> String {
    match entry
        .stated_by
        .as_ref()
        .filter(|by| by.kind == crate::AuthorKind::Agent)
    {
        Some(by) => agent_stated_whose(user, by.name.as_str()),
        None => stated_whose_for(user, speaker_of(entry)),
    }
}

/// The label before the user's own words beside such a statement.
#[must_use]
pub fn stated_quote_label(user: &UserName) -> String {
    format!("{}'s words:", user.title())
}

/// The label before a quote of `speaker`'s words beside scribe's
/// statement of their turn.
#[must_use]
pub fn words_label(user: &UserName, speaker: Speaker) -> String {
    match speaker {
        Speaker::User => stated_quote_label(user),
        Speaker::Assistant => String::from("The assistant's words:"),
    }
}

/// Whose turn an entry's content states: the assistant's only for a
/// message entry from an assistant turn.
#[must_use]
pub fn speaker_of(entry: &Entry) -> Speaker {
    match entry.content_from {
        ContentFrom::Message { role, .. } => role,
        ContentFrom::Agent | ContentFrom::Instruction => Speaker::User,
    }
}

/// Whose words an entry the session pass captured itself under a `write`
/// catcher is (2_design.md §11.6, §15.2): nobody typed or approved it, and
/// an assistant turn's words are never the user's.
#[must_use]
pub fn captured_whose(user: &UserName, speaker: Speaker) -> String {
    let noun = user.noun();
    match speaker {
        Speaker::User => format!(
            "scribe's statement of {noun}'s turn, captured by the session pass (not typed by {noun})"
        ),
        Speaker::Assistant => format!(
            "scribe's statement of the assistant's turn, captured by the session pass (not {noun}'s words)"
        ),
    }
}

/// The label of a claim quoting the captured turn's words within such an
/// entry.
#[must_use]
pub fn captured_user(user: &UserName, speaker: Speaker) -> String {
    match speaker {
        Speaker::User => format!("{} (captured by the session pass)", user.title()),
        Speaker::Assistant => String::from("assistant (captured by the session pass)"),
    }
}

/// The speaker's verbatim words beside an entry whose content is scribe's
/// or an agent's statement of their turn: one the user typed/marked,
/// approved, or spoke on instruction, or one the pass captured; `None` for
/// every other entry.
pub(crate) fn stated_quote(entry: &Entry) -> Option<&str> {
    entry
        .stated_by
        .as_ref()
        .and(entry.quote.as_deref())
        .filter(|_| {
            entry.author.kind == crate::AuthorKind::User
                || is_pass_captured(entry)
                || is_stated_assistant(entry)
                || is_agent_stated(entry)
        })
}

/// Whether `entry` is an agent-written entry on the user's spoken
/// instruction (2_design.md §16.6): `content` is the agent's statement and
/// `quote` is the user's verbatim words.
fn is_agent_stated(entry: &Entry) -> bool {
    entry.author.kind == crate::AuthorKind::Agent
        && entry.content_from == ContentFrom::Agent
        && entry
            .stated_by
            .as_ref()
            .is_some_and(|by| by.kind == crate::AuthorKind::Agent)
}

/// Whether `entry` is an approved non-finding assistant claim whose
/// content is scribe's context of the assistant's turn (2_design.md §15.3).
fn is_stated_assistant(entry: &Entry) -> bool {
    entry.author.kind == crate::AuthorKind::Agent
        && !entry.is_finding()
        && matches!(
            entry.content_from,
            ContentFrom::Message {
                role: Speaker::Assistant,
                ..
            }
        )
}

/// Whether the session pass wrote `entry` itself (an idea under a `write`
/// tag), rather than the user typing or approving it.
#[must_use]
pub fn is_pass_captured(entry: &Entry) -> bool {
    entry.author.kind == crate::AuthorKind::Scribe && entry.outcome.is_some()
}

/// An idea's outcome as one line of evidence or snippet text: `open`,
/// `parked, in the user's words: "<reason>"` or `task <id>`.
#[must_use]
pub fn outcome_text(outcome: &crate::Outcome, user: &UserName) -> String {
    match (outcome.state, &outcome.reason, &outcome.task) {
        (crate::OutcomeState::Parked, Some(reason), _) => {
            format!("parked, in {}'s words: \"{reason}\"", user.noun())
        }
        (crate::OutcomeState::Task, _, Some(task)) => format!("task {task}"),
        (state, ..) => state.as_str().to_owned(),
    }
}

/// Whose instruction a line the user reads names: `entry show`, `record
/// search` and `proposal list` speak to them.
pub const INSTRUCTION_YOURS: &str = "your";

/// Whose instruction a line in `ask` evidence names: the model reads it.
#[must_use]
pub fn instruction_theirs(user: &UserName) -> String {
    format!("{}'s", user.noun())
}

/// How a settlement on the user's checked instruction reads (2_design.md
/// §12.3): `approved by <aide> on your instruction "<quote>" (<session>,
/// <date>)`. `verb` is `approved` or `rejected`; `whose` is
/// [`INSTRUCTION_YOURS`] or [`instruction_theirs`]. A forgotten entry's
/// instruction has no quote, and the line then names only the turn.
#[must_use]
pub fn instructed_line(
    verb: &str,
    aide: &str,
    whose: &str,
    quote: Option<&str>,
    session: &str,
    at: OffsetDateTime,
) -> String {
    let quoted = quote
        .map(|q| {
            format!(
                " \"{}\"",
                q.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        })
        .unwrap_or_default();
    format!(
        "{verb} by {aide} on {whose} instruction{quoted} ({session}, {})",
        day(at)
    )
}

/// The approved-by line of an entry an agent approved on the user's
/// instruction; `None` for one they approved by typing, or never approved.
#[must_use]
pub fn approved_line(entry: &Entry, whose: &str) -> Option<String> {
    let by = entry.approved_by.as_ref()?;
    let inst = entry.instruction.as_ref()?;
    Some(instructed_line(
        "approved",
        &by.name.to_string(),
        whose,
        inst.quote.as_deref(),
        inst.session.as_str(),
        inst.at,
    ))
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

fn task_item(task: &Task, description: &str) -> EvidenceItem {
    EvidenceItem {
        cite: format!("task:{}", task.id),
        kind: "task",
        whose: "Orbit task".to_owned(),
        status: task.status.clone(),
        date: task.updated.map(day).unwrap_or_default(),
        text: format!("{}\n{description}", task.title),
        quote: None,
        approval: None,
    }
}
