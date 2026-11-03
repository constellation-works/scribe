//! A session pass: from a switched-on session's transcript to its distilled
//! session record (docs/design/scribe/2_design.md §5.3).
//!
//! The steps here are pure; the `session pass` command reads the transcript,
//! calls the `session` model through the [`crate::Models`] port and has the
//! store write the result:
//!
//! 1. [`conversation`]: the transcript's messages from the first one (the
//!    whole session, not only those since `on_since`), synthetic turns
//!    (`<task-notification>`, command echoes, `/scribe` itself) dropped and
//!    pasted blocks set apart from Daniel's own words;
//! 2. [`window`]: the messages after the record's `scribed_through`, at most
//!    [`WINDOW_CHARS`] characters, oldest dropped first and counted;
//! 3. [`request`]: the model request, with the previous record and the
//!    store's existing topics and terms;
//! 4. [`check`]: the answer's budgets and shape, refused whole when wrong
//!    (the caller keeps the previous record), and every term kept only when
//!    its `said` is a verbatim span of one of Daniel's own turns;
//! 5. [`record`]: the new record, `scribed_through` advanced.
//!
//! A pass writes no tag entries, whatever the transcript holds. No I/O
//! (STD-02 R5).

use crate::control;
use crate::entry::Speaker;
use crate::models::{
    Budgets, CUT_MARK, PassMessage, PreviousRecord, SESSION_BUDGET, SessionAnswer, SessionRequest,
};
use crate::record::{Label, Project, Related, Topic, Topics};
use crate::session::{SECTIONS, SessionId, SessionRecord, SessionStatus, Term};
use crate::transcript::Message;
use crate::verbatim;
use time::OffsetDateTime;

/// The most characters of new messages one pass sends (2_design.md §5.3).
pub const WINDOW_CHARS: usize = 150_000;

/// The most sessions one `session pass` run processes (2_design.md §5.2).
pub const MAX_PER_RUN: usize = 5;

/// Failed passes in a row after which `due` is cleared (2_design.md §5.3).
pub const MAX_FAILURES: u32 = 3;

/// The longest term, in characters.
pub const TERM_CHARS: usize = 80;

/// The longest `said` span of a term, in characters.
pub const SAID_CHARS: usize = 400;

/// The character budgets of a record (2_design.md §3): the title, then the
/// four sections as [`SECTIONS`] lists them.
pub const BUDGETS: Budgets = Budgets {
    title: 120,
    summary: SECTIONS[0].1,
    goals: SECTIONS[1].1,
    outcome: SECTIONS[2].1,
    next: SECTIONS[3].1,
};

/// User turns that Claude Code writes itself, not Daniel: a turn whose text
/// starts with one of these tags is dropped.
const SYNTHETIC: &[&str] = &[
    "<task-notification>",
    "<command-name>",
    "<command-message>",
    "<command-args>",
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<local-command-caveat>",
    "<system-reminder>",
    "<bash-input>",
    "<bash-stdout>",
    "<bash-stderr>",
    "<user-prompt-submit-hook>",
];

const PASTED_OPEN: &str = "<pasted_content";
const PASTED_CLOSE: &str = "</pasted_content>";

/// The session's messages as a pass reads them, in transcript order.
#[must_use]
pub fn conversation(messages: &[Message]) -> Vec<PassMessage> {
    messages
        .iter()
        .filter_map(|m| {
            let text = m.text();
            match m.role {
                Speaker::Assistant => {
                    let text = text.trim().to_owned();
                    (!text.is_empty()).then(|| PassMessage {
                        uuid: m.uuid.clone(),
                        role: Speaker::Assistant,
                        text,
                        pasted: Vec::new(),
                    })
                }
                Speaker::User => {
                    let start = text.trim_start();
                    if SYNTHETIC.iter().any(|tag| start.starts_with(tag))
                        || control::parse(&text).is_some()
                    {
                        return None;
                    }
                    let (own, pasted) = split_pasted(&text);
                    (!own.is_empty() || !pasted.is_empty()).then(|| PassMessage {
                        uuid: m.uuid.clone(),
                        role: Speaker::User,
                        text: own,
                        pasted,
                    })
                }
            }
        })
        .collect()
}

/// A user turn's own words and the blocks pasted into it. An unclosed block
/// runs to the end of the turn.
fn split_pasted(text: &str) -> (String, Vec<String>) {
    let mut own = String::new();
    let mut pasted = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find(PASTED_OPEN) {
        own.push_str(&rest[..open]);
        let after = &rest[open..];
        let body_start = after.find('>').map_or(after.len(), |i| i + 1);
        let body = &after[body_start..];
        match body.find(PASTED_CLOSE) {
            Some(close) => {
                pasted.push(body[..close].trim().to_owned());
                rest = &body[close + PASTED_CLOSE.len()..];
            }
            None => {
                pasted.push(body.trim().to_owned());
                rest = "";
            }
        }
    }
    own.push_str(rest);
    pasted.retain(|p| !p.is_empty());
    (own.trim().to_owned(), pasted)
}

/// How many of Daniel's turns the conversation holds after the message
/// `through` (all of them when `through` is `None` or not found).
#[must_use]
pub fn user_turns_after(conversation: &[PassMessage], through: Option<&str>) -> u32 {
    let new = &conversation[start_after(conversation, through)..];
    u32::try_from(new.iter().filter(|m| m.role == Speaker::User).count()).unwrap_or(u32::MAX)
}

fn start_after(conversation: &[PassMessage], through: Option<&str>) -> usize {
    through
        .and_then(|uuid| conversation.iter().position(|m| m.uuid == uuid))
        .map_or(0, |i| i + 1)
}

/// The messages one pass sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// The messages after the last pass, oldest first.
    pub messages: Vec<PassMessage>,
    /// How many older new messages were left out to fit.
    pub dropped: usize,
}

/// The messages after `through` (the record's `scribed_through`; all of
/// them when it is `None` or no longer in the transcript), at most
/// `max_chars` characters: the oldest are dropped first, and a single
/// message longer than the window is cut, the cut marked.
#[must_use]
pub fn window(conversation: &[PassMessage], through: Option<&str>, max_chars: usize) -> Window {
    let mut messages: Vec<PassMessage> =
        conversation[start_after(conversation, through)..].to_vec();
    let mut total: usize = messages.iter().map(PassMessage::chars).sum();
    let mut dropped = 0;
    while total > max_chars && messages.len() > 1 {
        total -= messages.remove(0).chars();
        dropped += 1;
    }
    if let Some(only) = messages.first_mut().filter(|m| m.chars() > max_chars) {
        only.pasted.clear();
        only.text = cut(&only.text, max_chars);
    }
    Window { messages, dropped }
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str(CUT_MARK);
    out
}

/// The session-model request for one pass.
#[must_use]
pub fn request(
    model: &str,
    previous: Option<&SessionRecord>,
    window: Window,
    topics: Vec<String>,
    terms: Vec<String>,
    final_pass: bool,
) -> SessionRequest {
    SessionRequest {
        model: model.to_owned(),
        budget: SESSION_BUDGET,
        previous: previous.map(|r| PreviousRecord {
            title: r.title.clone(),
            summary: r.summary.clone(),
            goals: r.goals.clone(),
            outcome: r.outcome.clone(),
            next: r.next.clone(),
            topics: r
                .topics
                .as_slice()
                .iter()
                .map(ToString::to_string)
                .collect(),
        }),
        messages: window.messages,
        dropped: window.dropped,
        topics,
        terms,
        budgets: BUDGETS,
        final_pass,
    }
}

/// Why an answer was refused; the previous record is kept and the pass is
/// retried.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    /// The title is empty or not one line.
    #[error("the title must be one non-empty line")]
    Title,
    /// A field is over its character budget.
    #[error("{field} has {chars} characters; the budget is {max}")]
    OverBudget {
        /// The field.
        field: &'static str,
        /// Its length.
        chars: usize,
        /// Its budget.
        max: usize,
    },
    /// The summary is empty.
    #[error("the summary is empty")]
    EmptySummary,
    /// Section text carries a line that would read as another section's
    /// heading.
    #[error("{0} holds a section heading line")]
    Heading(&'static str),
    /// The topics are not 0-5 distinct lowercase kebab-case labels.
    #[error("bad topics: {0}")]
    Topics(String),
}

/// A checked answer, ready to become a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Distilled {
    /// The title.
    pub title: String,
    /// `## Summary`.
    pub summary: String,
    /// `## Goals`.
    pub goals: String,
    /// `## Outcome`.
    pub outcome: String,
    /// `## Next`, one `- <item>` line per item.
    pub next: String,
    /// How many items `next` lists.
    pub next_open: u32,
    /// Topics.
    pub topics: Topics,
    /// References that parsed; the rest are dropped.
    pub related: Vec<Related>,
    /// Terms whose `said` passed the verbatim check.
    pub terms: Vec<Term>,
    /// Terms dropped: not verbatim from one of Daniel's turns, or over a
    /// length limit.
    pub dropped_terms: usize,
}

/// Check `answer` against the budgets and shape, and keep each term only
/// when its `said` is a verbatim span of one of Daniel's own turns in
/// `conversation` (never an assistant turn or a pasted block). A bad
/// budget, title, section or topic refuses the whole answer; a bad
/// reference or term is dropped and the rest kept.
pub fn check(answer: SessionAnswer, conversation: &[PassMessage]) -> Result<Distilled, Invalid> {
    let title = answer.title.trim().to_owned();
    if title.is_empty() || title.contains('\n') {
        return Err(Invalid::Title);
    }
    within("title", &title, BUDGETS.title)?;
    let summary = section("Summary", &answer.summary, BUDGETS.summary)?;
    if summary.is_empty() {
        return Err(Invalid::EmptySummary);
    }
    let goals = section("Goals", &answer.goals, BUDGETS.goals)?;
    let outcome = section("Outcome", &answer.outcome, BUDGETS.outcome)?;
    let items: Vec<String> = answer
        .next
        .iter()
        .map(|item| item.split_whitespace().collect::<Vec<_>>().join(" "))
        .map(|item| item.trim_start_matches("- ").to_owned())
        .filter(|item| !item.is_empty())
        .collect();
    let next = items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n");
    within("next", &next, BUDGETS.next)?;

    let mut topics: Vec<Topic> = Vec::new();
    for raw in &answer.topics {
        let topic: Topic = raw
            .trim()
            .parse()
            .map_err(|e: crate::record::RecordError| Invalid::Topics(e.to_string()))?;
        if !topics.contains(&topic) {
            topics.push(topic);
        }
    }
    let topics = Topics::try_from(topics).map_err(|e| Invalid::Topics(e.to_string()))?;

    let mut related: Vec<Related> = Vec::new();
    for raw in answer.related {
        if let Ok(r) = Related::try_from(raw.trim().to_owned())
            && !related.contains(&r)
        {
            related.push(r);
        }
    }

    let mut terms: Vec<Term> = Vec::new();
    let mut dropped_terms = 0;
    for t in answer.terms {
        match verified_term(&t.term, &t.said, &t.message, conversation) {
            Some(term) if !terms.iter().any(|k| same_term(&k.term, &term.term)) => {
                terms.push(term);
            }
            Some(_) => {}
            None => dropped_terms += 1,
        }
    }

    Ok(Distilled {
        title,
        summary,
        goals,
        outcome,
        next,
        next_open: u32::try_from(items.len()).unwrap_or(u32::MAX),
        topics,
        related,
        terms,
        dropped_terms,
    })
}

fn within(field: &'static str, text: &str, max: usize) -> Result<(), Invalid> {
    let chars = text.chars().count();
    if chars > max {
        return Err(Invalid::OverBudget { field, chars, max });
    }
    Ok(())
}

/// A section's text, trimmed and within its budget, with no line that the
/// record parser would read as a section heading.
fn section(name: &'static str, text: &str, max: usize) -> Result<String, Invalid> {
    let text = text.trim().to_owned();
    within(name, &text, max)?;
    let heading = text.lines().any(|line| {
        line.strip_prefix("## ")
            .is_some_and(|h| SECTIONS.iter().any(|(s, _)| *s == h))
    });
    if heading {
        return Err(Invalid::Heading(name));
    }
    Ok(text)
}

/// The term, if its `said` is a verbatim span of one of Daniel's own turns:
/// the one the model named when it holds the span, else the first that
/// does. Its `message` is that turn's uuid.
fn verified_term(
    term: &str,
    said: &str,
    named: &str,
    conversation: &[PassMessage],
) -> Option<Term> {
    let term = term.split_whitespace().collect::<Vec<_>>().join(" ");
    let said = said.trim();
    if term.is_empty() || term.chars().count() > TERM_CHARS || said.chars().count() > SAID_CHARS {
        return None;
    }
    let holds = |m: &&PassMessage| m.role == Speaker::User && verbatim::contains(&m.text, said);
    let turn = conversation
        .iter()
        .filter(holds)
        .find(|m| m.uuid == named)
        .or_else(|| conversation.iter().find(holds))?;
    Some(Term {
        term,
        said: said.to_owned(),
        message: turn.uuid.clone(),
    })
}

/// Whether two terms name the same phrase: case and whitespace aside.
fn same_term(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// The record's terms after a pass: the previous ones, then each new term
/// not already among them.
#[must_use]
pub fn merge_terms(previous: &[Term], new: Vec<Term>) -> Vec<Term> {
    let mut out = previous.to_vec();
    for term in new {
        if !out.iter().any(|t| same_term(&t.term, &term.term)) {
            out.push(term);
        }
    }
    out
}

/// What a pass knows about the session besides the model's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassMeta {
    /// The session.
    pub id: SessionId,
    /// The repository its cwd belongs to.
    pub project: Project,
    /// The tool, `claude-code`.
    pub provider: Label,
    /// This host.
    pub host: Label,
    /// The session's working directory.
    pub cwd: String,
    /// The transcript's first message.
    pub started: OffsetDateTime,
    /// When cataloging was last switched on.
    pub on_since: OffsetDateTime,
    /// The last message folded in.
    pub through: Option<String>,
    /// Now.
    pub now: OffsetDateTime,
    /// Whether this is the session-end pass.
    pub final_pass: bool,
}

/// The record a pass writes. A previous record keeps its `created`,
/// `started`, `cwd`, `project`, `provider` and `host` (the file stays where
/// it is) and its terms, the new ones appended; `passes` counts up and
/// `scribed_through` advances to the last message the pass read.
#[must_use]
pub fn record(
    previous: Option<&SessionRecord>,
    distilled: Distilled,
    meta: PassMeta,
) -> SessionRecord {
    let terms = merge_terms(
        previous.map_or(&[][..], |p| p.terms.as_slice()),
        distilled.terms,
    );
    SessionRecord {
        id: meta.id,
        status: if meta.final_pass {
            SessionStatus::Final
        } else {
            SessionStatus::Open
        },
        project: previous.map_or(meta.project, |p| p.project.clone()),
        topics: distilled.topics,
        related: distilled.related,
        terms,
        provider: previous.map_or(meta.provider, |p| p.provider.clone()),
        host: previous.map_or(meta.host, |p| p.host.clone()),
        created: previous.map_or(meta.now, |p| p.created),
        updated: meta.now,
        title: distilled.title,
        cwd: previous.map_or(meta.cwd, |p| p.cwd.clone()),
        started: previous.map_or(meta.started, |p| p.started),
        on_since: meta.on_since,
        scribed_through: meta
            .through
            .or_else(|| previous.and_then(|p| p.scribed_through.clone())),
        passes: previous.map_or(0, |p| p.passes).saturating_add(1),
        next_open: distilled.next_open,
        summary: distilled.summary,
        goals: distilled.goals,
        outcome: distilled.outcome,
        next: distilled.next,
    }
}

/// The session ended with nothing new since the last pass: the same record,
/// marked final, without a model call.
#[must_use]
pub fn finalized(previous: &SessionRecord, now: OffsetDateTime) -> SessionRecord {
    let mut record = previous.clone();
    record.status = SessionStatus::Final;
    record.updated = now;
    record
}
