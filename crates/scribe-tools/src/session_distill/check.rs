//! Deterministic checks on a session activity answer and its citation slices.

mod repair;

use super::{
    Budgets, NewSegmentAnswer, PassMessage, SegmentAnswer, SessionAnswer, SessionHeader,
    SessionHeaderRequest, SessionRequest,
};
use crate::text::verbatim;
use crate::voice::Voice;
pub use repair::repair;
use scribe_store::entry::Speaker;
use scribe_store::record::{Related, Topic, Topics};
use scribe_store::session::{SEGMENT_HEADING_CHARS, Term};

/// The longest term, in characters.
pub const TERM_CHARS: usize = 80;

/// The longest `said` span of a term, in characters.
pub const SAID_CHARS: usize = 400;

/// Header character budgets (2_design.md §5.5).
pub const BUDGETS: Budgets = Budgets {
    title: 120,
    summary: 1200,
    goals: 600,
    next: 1000,
};

/// Maximum text length of a model-written segment.
pub const SEGMENT_CHARS: usize = 900;
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
    /// Segment presence, content or message range is invalid.
    #[error("{0}")]
    Segment(String),
}

/// A checked answer, ready to become a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Distilled {
    /// Candidate answers to check against the capture rules in the session app.
    pub candidates: Vec<super::EntryCandidateAnswer>,
    /// The model's memory elements, unchecked: the caller checks them with
    /// its memory gate against the config and redactor.
    pub memory: Vec<super::MemoryAnswer>,
    /// How much the window says about who the user is, 0–3, when memory
    /// was asked (2_design.md §15.6).
    pub signal: Option<u8>,
    /// The title.
    pub title: String,
    /// `## Summary`.
    pub summary: String,
    /// `## Goals`.
    pub goals: String,
    /// The rewritten open segment.
    pub open: Option<CheckedSegment>,
    /// New segments with their starting offsets in the window.
    pub new: Vec<(usize, CheckedSegment)>,
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
    /// Deterministic repairs made after the model retry; no model prose.
    pub repairs: Vec<String>,
    /// An overlong open text with no successor seals at this window's end.
    pub seal_open: bool,
}

/// Validated content of a segment; message ranges are derived by scribe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedSegment {
    /// One-line heading.
    pub heading: String,
    /// Plain text.
    pub text: String,
    /// Checked topic labels.
    pub topics: Topics,
    /// Parsed references.
    pub related: Vec<Related>,
}

/// Check `answer` against the budgets and shape, and keep each term only
/// when its `said` is a verbatim span of one of Daniel's own turns in
/// `conversation` (never an assistant turn or a pasted block). A bad
/// budget, title, section or topic refuses the whole answer; a bad
/// reference or term is dropped and the rest kept.
pub fn check(
    answer: SessionAnswer,
    conversation: &[PassMessage],
    request: &SessionRequest,
) -> Result<Distilled, Invalid> {
    check_with_budgets(answer, conversation, request, BUDGETS)
}

/// A header-only retry, only when every non-length check already passes.
/// The caller keeps the original answer and replaces only its prose header,
/// then checks the entire answer again with [`check`]. `voice` names the
/// user and carries `[instructions] session_header`.
#[must_use]
pub fn compression_request(
    answer: &SessionAnswer,
    conversation: &[PassMessage],
    request: &SessionRequest,
    voice: Voice,
) -> Option<SessionHeaderRequest> {
    let refusal = check(answer.clone(), conversation, request).err()?;
    if !matches!(
        refusal,
        Invalid::OverBudget {
            field: "title" | "Summary" | "Goals" | "next",
            ..
        }
    ) {
        return None;
    }
    // Ignore only header lengths, never headings, topics, segment limits
    // or ranges. A hidden second defect needs a full-window correction.
    check_with_budgets(
        answer.clone(),
        conversation,
        request,
        Budgets {
            title: usize::MAX,
            summary: usize::MAX,
            goals: usize::MAX,
            next: usize::MAX,
        },
    )
    .ok()?;
    Some(SessionHeaderRequest {
        model: request.model.clone(),
        budget: request.budget,
        header: SessionHeader {
            title: answer.title.clone(),
            summary: answer.summary.clone(),
            goals: answer.goals.clone(),
            next: answer.next.clone(),
        },
        budgets: BUDGETS,
        correction: refusal.to_string(),
        voice,
    })
}

fn check_with_budgets(
    answer: SessionAnswer,
    conversation: &[PassMessage],
    request: &SessionRequest,
    budgets: Budgets,
) -> Result<Distilled, Invalid> {
    check_with_rules(answer, conversation, request, budgets, false)
}

/// Relax only the rules the deterministic repair owns, checking the original
/// uncut prose so a cut cannot hide an invalid heading or topic.
fn check_with_rules(
    mut answer: SessionAnswer,
    conversation: &[PassMessage],
    request: &SessionRequest,
    budgets: Budgets,
    repairable: bool,
) -> Result<Distilled, Invalid> {
    let title = answer.title.trim().to_owned();
    if title.is_empty() || title.contains(['\n', '\r']) {
        return Err(Invalid::Title);
    }
    within("title", &title, budgets.title)?;
    let summary = section("Summary", &answer.summary, budgets.summary)?;
    if summary.is_empty() {
        return Err(Invalid::EmptySummary);
    }
    let goals = section("Goals", &answer.goals, budgets.goals)?;
    let expected = request.previous.as_ref().and_then(|p| p.open.as_ref());
    let open = match (expected, answer.open) {
        (Some(previous), None) => Some(CheckedSegment {
            heading: previous.heading.clone(),
            text: previous.text.clone(),
            topics: previous.topics.clone(),
            related: previous.related.clone(),
        }),
        (Some(_), Some(open)) => Some(check_segment(open, repairable)?),
        (None, None) => None,
        (None, Some(open)) => {
            let first = request.messages.first().ok_or_else(|| {
                Invalid::Segment(String::from(
                    "unexpected open cannot start a new segment in an empty window",
                ))
            })?;
            answer.new.insert(
                0,
                NewSegmentAnswer {
                    starts_at: first.uuid.clone(),
                    heading: open.heading,
                    text: open.text,
                    topics: open.topics,
                    related: open.related,
                },
            );
            None
        }
    };
    if answer.new.len() > 3 {
        return Err(Invalid::Segment(String::from(
            "new must hold at most 3 segments",
        )));
    }
    let mut new = Vec::new();
    let mut last_start = None;
    let mut seen_starts = std::collections::BTreeSet::new();
    for segment in answer.new {
        let start = request
            .messages
            .iter()
            .position(|m| m.uuid == segment.starts_at);
        if start.is_none() && !repairable {
            return Err(Invalid::Segment(format!(
                "starts_at '{}' is outside the window",
                segment.starts_at
            )));
        }
        if !seen_starts.insert(segment.starts_at.clone())
            || start.is_some_and(|start| last_start.is_some_and(|last| start <= last))
        {
            return Err(Invalid::Segment(String::from(
                "starts_at ids must be distinct and in transcript order",
            )));
        }
        if !repairable
            && let Some(last) = expected
                .and_then(|s| s.last_message.as_ref())
                .and_then(|id| conversation.iter().position(|m| &m.uuid == id))
            && conversation
                .iter()
                .position(|m| m.uuid == segment.starts_at)
                .is_none_or(|i| i <= last)
        {
            return Err(Invalid::Segment(String::from(
                "starts_at must follow the open segment's last message",
            )));
        }
        if let Some(start) = start {
            last_start = Some(start);
        }
        let checked = check_segment(
            SegmentAnswer {
                heading: segment.heading,
                text: segment.text,
                topics: segment.topics,
                related: segment.related,
            },
            repairable,
        )?;
        // This sentinel is used only in the discarded repair eligibility check.
        new.push((start.unwrap_or(usize::MAX), checked));
    }
    if expected.is_none()
        && new
            .first()
            .is_none_or(|(start, _)| *start != 0 && !(repairable && *start == usize::MAX))
    {
        return Err(Invalid::Segment(String::from(
            "without an open segment, new must start at the window's first message",
        )));
    }
    let items = next_items(&answer.next);
    let next = items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n");
    within("next", &next, budgets.next)?;

    let mut topics: Vec<Topic> = Vec::new();
    for raw in &answer.topics {
        let topic: Topic = raw
            .trim()
            .parse()
            .map_err(|e: scribe_store::record::RecordError| Invalid::Topics(e.to_string()))?;
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

    // Only as many as the request asked for; none when it asked for none.
    let memory = answer
        .memory
        .into_iter()
        .take(request.memory_limit)
        .collect();
    // The signal only when asked, clamped to 0-3 (2_design.md §15.6).
    let signal = answer.signal.filter(|_| request.memory).map(|s| s.min(3));
    Ok(Distilled {
        candidates: answer.candidates,
        memory,
        signal,
        title,
        summary,
        goals,
        open,
        new,
        next,
        next_open: u32::try_from(items.len()).unwrap_or(u32::MAX),
        topics,
        related,
        terms,
        dropped_terms,
        repairs: Vec::new(),
        seal_open: false,
    })
}

fn check_segment(answer: SegmentAnswer, repairable: bool) -> Result<CheckedSegment, Invalid> {
    let heading = answer.heading.trim().to_owned();
    within("segment heading", &heading, SEGMENT_HEADING_CHARS)?;
    if heading.is_empty() || heading.contains(['\n', '\r']) {
        return Err(Invalid::Segment(String::from(
            "a segment heading must be one non-empty line",
        )));
    }
    let text = answer.text.trim().to_owned();
    if !repairable && text.chars().count() > SEGMENT_CHARS {
        return Err(Invalid::Segment(format!(
            "segment text has {} characters; the budget is {SEGMENT_CHARS}; seal it and start a new segment",
            text.chars().count()
        )));
    }
    if text.is_empty() || text.lines().any(|l| l.trim_start().starts_with('#')) {
        return Err(Invalid::Segment(String::from(
            "segment text must have 1-900 characters and no # heading line",
        )));
    }
    let mut topics = Vec::new();
    for raw in answer.topics {
        let topic: Topic = raw
            .parse()
            .map_err(|e: scribe_store::record::RecordError| Invalid::Topics(e.to_string()))?;
        if !topics.contains(&topic) {
            topics.push(topic);
        }
    }
    let topics = Topics::try_from(topics).map_err(|e| Invalid::Topics(e.to_string()))?;
    let mut related = Vec::new();
    for raw in answer.related {
        if let Ok(r) = Related::try_from(raw.trim().to_owned())
            && !related.contains(&r)
        {
            related.push(r);
        }
    }
    Ok(CheckedSegment {
        heading,
        text,
        topics,
        related,
    })
}

fn within(field: &'static str, text: &str, max: usize) -> Result<(), Invalid> {
    let chars = text.chars().count();
    if chars > max {
        return Err(Invalid::OverBudget { field, chars, max });
    }
    Ok(())
}

fn next_items(next: &[String]) -> Vec<String> {
    next.iter()
        .map(|item| item.split_whitespace().collect::<Vec<_>>().join(" "))
        .map(|item| item.trim_start_matches("- ").to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

/// A section's text, trimmed and within its budget, with no line that the
/// record parser would read as a section or segment heading.
fn section(name: &'static str, text: &str, max: usize) -> Result<String, Invalid> {
    let text = text.trim().to_owned();
    within(name, &text, max)?;
    let heading = text
        .lines()
        .any(|line| line.trim_start().starts_with("## ") || line.trim_start().starts_with("### "));
    if heading {
        return Err(Invalid::Heading(name));
    }
    Ok(text)
}

/// The term, only if its named message is Daniel's own turn and holds `said`.
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
        .find(|m| m.uuid == named)?;
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
