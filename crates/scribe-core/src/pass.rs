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
//! 2. [`window_with_limits`]: the next consecutive messages after the record's
//!    `scribed_through`, within the configured message and character limits;
//! 3. [`request`]: the model request, with the previous record, the
//!    latest records of the sessions this one links to ([`linked_context`]:
//!    context only, the prompt says not to retell them) and the store's
//!    existing topics and terms;
//! 4. [`check`]: the answer's budgets and shape, with [`repair`] available
//!    after the caller's one retry for bounded prose and start misses;
//!    every term kept only when
//!    its `said` is a verbatim span of one of Daniel's own turns;
//! 5. [`record`]: the new record, `scribed_through` advanced.
//!
//! A window rewrites the header and open segment, then appends up to three
//! new segments. Sealed text is never sent to the model or rewritten.
//!
//! Claims pass two gates (§11.2): a model-free trigger match on Daniel's
//! turns before the call ([`capture_request`]: no trigger fires, no claim
//! is requested) and the tag's criteria, judged by the model; the store
//! repeats the trigger match under its lock. A claim under a `propose` tag
//! opens a candidate proposal; one under a `write` tag (`idea`, §11.6) is
//! written by the store as an active entry the pass authors, with its
//! outcome, at most [`MAX_IDEAS`] per session per invocation.
//!
//! The pass writes no other entries, whatever the transcript holds. No I/O
//! (STD-02 R5).

use crate::entry::Speaker;
use crate::models::{
    Budgets, CUT_MARK, LinkedRecord, NewSegmentAnswer, PassMessage, PreviousRecord, SESSION_BUDGET,
    SegmentAnswer, SegmentHeading, SessionAnswer, SessionHeader, SessionHeaderRequest,
    SessionRequest,
};
use crate::record::{Label, Project, Related, Topic, Topics};
use crate::session::{
    SEGMENT_HEADING_CHARS, Segment, SegmentOrigin, SegmentState, SessionId, SessionLink,
    SessionRecord, SessionStatus, Term,
};
use crate::transcript::Message;
use crate::verbatim;
use std::num::NonZeroU32;
use time::OffsetDateTime;

mod candidates;
#[cfg(test)]
pub(crate) use candidates::covers_statement;
pub(crate) use candidates::own_lines;
pub use candidates::{
    MAX_CANDIDATES, MAX_IDEAS, MAX_OPEN_CANDIDATES, PassClaim, candidate_drafts,
    candidate_exclusions, capture_request, write_exclusions,
};
pub(crate) use candidates::{admission, is_marker, own_marked_span, own_span, statement_tokens};
pub(crate) use candidates::{
    candidate_says, claim_covers, covering_claim, covering_entry, entry_says,
};
mod repair;
pub use repair::repair;

/// The default characters of message content in one window (2_design.md §5.3).
pub const WINDOW_CHARS: usize = 15_000;

/// The default number of messages in one window.
pub const WINDOW_MESSAGES: usize = 40;

/// Bounds on each consecutive model window, configured under `[catalog]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowLimits {
    /// The most messages in one call.
    pub messages: std::num::NonZeroUsize,
    /// The most Unicode characters of message content in one call.
    pub chars: std::num::NonZeroUsize,
}

impl Default for WindowLimits {
    fn default() -> Self {
        Self {
            messages: std::num::NonZeroUsize::new(WINDOW_MESSAGES)
                .unwrap_or(std::num::NonZeroUsize::MIN),
            chars: std::num::NonZeroUsize::new(WINDOW_CHARS).unwrap_or(std::num::NonZeroUsize::MIN),
        }
    }
}

/// The most sessions one `session pass` run processes (2_design.md §5.2).
pub const MAX_PER_RUN: usize = 5;

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
/// Default windows before sealing an open segment.
pub const SEGMENT_WINDOWS: NonZeroU32 = match NonZeroU32::new(3) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};

/// The session's messages as a pass reads them, in transcript order.
/// Daniel's turns are [`crate::transcript::user_turn`].
#[must_use]
pub fn conversation(messages: &[Message]) -> Vec<PassMessage> {
    messages
        .iter()
        .filter_map(|m| match m.role {
            Speaker::Assistant => {
                let text = m.text().trim().to_owned();
                (!text.is_empty()).then(|| PassMessage {
                    uuid: m.uuid.clone(),
                    role: Speaker::Assistant,
                    text,
                    pasted: Vec::new(),
                })
            }
            Speaker::User => {
                let turn = crate::transcript::user_turn(m)?;
                Some(PassMessage {
                    uuid: m.uuid.clone(),
                    role: Speaker::User,
                    text: turn.own,
                    pasted: turn.pasted,
                })
            }
        })
        .collect()
}

/// How many of Daniel's turns the conversation holds after the message
/// `through` (all of them when `through` is `None` or not found).
#[must_use]
pub fn user_turns_after(conversation: &[PassMessage], through: Option<&str>) -> u32 {
    let new = &conversation[start_after(conversation, through)..];
    u32::try_from(new.iter().filter(|m| m.role == Speaker::User).count()).unwrap_or(u32::MAX)
}

/// Index after the persisted cursor, or the start when that cursor is absent.
#[must_use]
pub fn start_after(conversation: &[PassMessage], through: Option<&str>) -> usize {
    through
        .and_then(|uuid| conversation.iter().position(|m| m.uuid == uuid))
        .map_or(0, |i| i + 1)
}

/// The messages one pass sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// The messages after the last pass, oldest first.
    pub messages: Vec<PassMessage>,
    /// Kept for request compatibility; consecutive windows never drop messages.
    pub dropped: usize,
}

/// The messages after `through` (the record's `scribed_through`; all of
/// them when it is `None` or no longer in the transcript), oldest first,
/// with the default message count and at most `max_chars` characters.
#[must_use]
pub fn window(conversation: &[PassMessage], through: Option<&str>, max_chars: usize) -> Window {
    next_window(conversation, through, WINDOW_MESSAGES, max_chars)
}

/// The next oldest window. No message is skipped; an individually oversized
/// message is cut with a marker, including that marker in the character bound.
#[must_use]
pub fn window_with_limits(
    conversation: &[PassMessage],
    through: Option<&str>,
    limits: WindowLimits,
) -> Window {
    next_window(
        conversation,
        through,
        limits.messages.get(),
        limits.chars.get(),
    )
}

fn next_window(
    conversation: &[PassMessage],
    through: Option<&str>,
    max_messages: usize,
    max_chars: usize,
) -> Window {
    let mut messages = Vec::new();
    let mut total = 0usize;
    for message in conversation[start_after(conversation, through)..]
        .iter()
        .take(max_messages)
    {
        let chars = message.chars();
        if !messages.is_empty() && chars > max_chars.saturating_sub(total) {
            break;
        }
        let mut message = message.clone();
        if chars > max_chars {
            // Keep the speaker's own text and pasted blocks distinct, even at a cut.
            let marker: String = CUT_MARK.chars().take(max_chars).collect();
            let mut left = max_chars.saturating_sub(marker.chars().count());
            message.text = message.text.chars().take(left).collect();
            left = left.saturating_sub(message.text.chars().count());
            message.pasted = message
                .pasted
                .iter()
                .filter_map(|p| {
                    if left == 0 {
                        return None;
                    }
                    let text: String = p.chars().take(left).collect();
                    left = left.saturating_sub(text.chars().count());
                    Some(text)
                })
                .collect();
            match message.pasted.last_mut() {
                Some(pasted) => pasted.push_str(&marker),
                None => message.text.push_str(&marker),
            }
        }
        total = total.saturating_add(message.chars());
        messages.push(message);
    }
    Window {
        messages,
        dropped: 0,
    }
}

/// The latest records of the sessions `links` point at, as pass context,
/// in link order; a link whose session has no record in `records` is left
/// out.
#[must_use]
pub fn linked_context(links: &[SessionLink], records: &[SessionRecord]) -> Vec<LinkedRecord> {
    links
        .iter()
        .filter_map(|link| {
            let record = records.iter().find(|r| r.id == link.session)?;
            Some(LinkedRecord {
                kind: link.kind,
                session: record.id.to_string(),
                title: record.title.clone(),
                summary: record.summary.clone(),
            })
        })
        .collect()
}

/// The session-model request for one pass. `linked` is the context of the
/// sessions this one links to ([`linked_context`]); `voice` names the user
/// and carries `[instructions] session`.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "each pass input is a separate argument named at the call site; a struct would only move them"
)]
pub fn request(
    model: &str,
    previous: Option<&SessionRecord>,
    linked: Vec<LinkedRecord>,
    window: Window,
    topics: Vec<String>,
    terms: Vec<String>,
    final_pass: bool,
    voice: crate::voice::Voice,
) -> SessionRequest {
    SessionRequest {
        voice,
        model: model.to_owned(),
        budget: SESSION_BUDGET,
        previous: previous.map(|r| PreviousRecord {
            title: r.title.clone(),
            summary: r.summary.clone(),
            goals: r.goals.clone(),
            open: r.open_segment().cloned(),
            sealed: r
                .log
                .iter()
                .filter(|s| s.state == SegmentState::Sealed)
                .map(|s| SegmentHeading {
                    n: s.n.get(),
                    heading: s.heading.clone(),
                    from: s.from,
                    to: s.to,
                })
                .collect(),
            terms: r.terms.clone(),
            next: r.next.clone(),
            topics: r
                .topics
                .as_slice()
                .iter()
                .map(ToString::to_string)
                .collect(),
        }),
        linked,
        messages: window.messages,
        dropped: window.dropped,
        topics,
        terms,
        candidate_limit: 0,
        candidate_turns: Vec::new(),
        candidate_rules: Vec::new(),
        candidate_exclusions: Vec::new(),
        write_limit: 0,
        write_exclusions: Vec::new(),
        memory_limit: 0,
        memory_sections: Vec::new(),
        memory_personal: false,
        memory_items: Vec::new(),
        budgets: BUDGETS,
        final_pass,
        correction: None,
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
    /// Segment presence, content or message range is invalid.
    #[error("{0}")]
    Segment(String),
}

/// A checked answer, ready to become a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Distilled {
    /// Locally checked candidate statements from this window.
    pub candidates: Vec<crate::models::EntryCandidateAnswer>,
    /// The model's memory elements, unchecked: the caller checks them with
    /// [`crate::memory::check`] against the config and redactor.
    pub memory: Vec<crate::models::MemoryAnswer>,
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
    voice: crate::voice::Voice,
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

    let candidates = candidates::check(answer.candidates, request);
    // Only as many as the request asked for; none when it asked for none.
    let memory = answer
        .memory
        .into_iter()
        .take(request.memory_limit)
        .collect();
    Ok(Distilled {
        candidates,
        memory,
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
            .map_err(|e: crate::record::RecordError| Invalid::Topics(e.to_string()))?;
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
    /// The last message folded in: the window's last.
    pub through: Option<String>,
    /// The window's first message.
    pub first_message: Option<String>,
    /// When the window's first message was sent.
    pub from: OffsetDateTime,
    /// When the window's last message was sent.
    pub to: OffsetDateTime,
    /// Now.
    pub now: OffsetDateTime,
    /// Whether this is the session-end pass.
    pub final_pass: bool,
    /// Window message ids and timestamps in transcript order.
    pub window: Vec<(String, OffsetDateTime)>,
    /// Windows before automatic sealing.
    pub segment_windows: NonZeroU32,
    /// The route writing new segments.
    pub origin: SegmentOrigin,
}

/// The record a pass writes. A previous record keeps its `created`,
/// `started`, `cwd`, `project`, `provider` and `host` (the file stays where
/// it is), its activity fields and catalog marker, its sealed segments and
/// its terms, the new ones appended; `passes` counts up and
/// `scribed_through` advances to the last message the pass read. The
/// header and checked segments become the next record ([`log_after`]).
#[must_use]
pub fn record(
    previous: Option<&SessionRecord>,
    distilled: Distilled,
    meta: PassMeta,
) -> SessionRecord {
    let log = log_after(previous, &distilled, &meta);
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
        active: previous.is_none_or(|p| p.active),
        keep_active: previous.is_some_and(|p| p.keep_active),
        active_change: previous.and_then(|p| p.active_change.clone()),
        cataloged_by: previous.and_then(|p| p.cataloged_by.clone()),
        rebuilt_by: previous.and_then(|p| p.rebuilt_by.clone()),
        project: previous.map_or(meta.project, |p| p.project.clone()),
        topics: distilled.topics,
        related: distilled.related,
        links: previous.map_or_else(Vec::new, |p| p.links.clone()),
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
        log,
        summary: distilled.summary,
        goals: distilled.goals,
        next: distilled.next,
    }
}

/// Extend the open segment and append new ones, deriving ranges from the
/// checked starts and transcript timestamps. Earlier sealed segments are cloned.
fn log_after(
    previous: Option<&SessionRecord>,
    distilled: &Distilled,
    meta: &PassMeta,
) -> Vec<Segment> {
    let mut log = previous.map_or_else(Vec::new, |p| p.log.clone());
    let end = meta.window.len().saturating_sub(1);
    if let Some(content) = &distilled.open
        && let Some(open) = log.last_mut().filter(|s| s.state == SegmentState::Open)
    {
        open.heading.clone_from(&content.heading);
        open.text.clone_from(&content.text);
        open.topics = content.topics.clone();
        open.related.clone_from(&content.related);
        open.windows = open.windows.saturating_add(1);
        let until = distilled
            .new
            .first()
            .map_or(Some(end), |(start, _)| start.checked_sub(1));
        if let Some((id, at)) = until.and_then(|i| meta.window.get(i)) {
            open.last_message = Some(id.clone());
            open.to = *at;
        }
        if !distilled.new.is_empty() {
            open.state = SegmentState::Sealed;
        }
    }
    for (i, (start, content)) in distilled.new.iter().enumerate() {
        let until = distilled
            .new
            .get(i + 1)
            .map_or(end, |(next, _)| next.saturating_sub(1));
        let first = meta.window.get(*start);
        let last = meta.window.get(until);
        let n = u32::try_from(log.len())
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        log.push(Segment {
            n: NonZeroU32::new(n).unwrap_or(NonZeroU32::MAX),
            heading: content.heading.clone(),
            text: content.text.clone(),
            first_message: first.map(|(id, _)| id.clone()),
            last_message: last.map(|(id, _)| id.clone()),
            from: first.map_or(meta.from, |(_, at)| *at),
            to: last.map_or(meta.to, |(_, at)| *at),
            windows: NonZeroU32::MIN,
            origin: meta.origin,
            state: if i + 1 < distilled.new.len() {
                SegmentState::Sealed
            } else {
                SegmentState::Open
            },
            topics: content.topics.clone(),
            related: content.related.clone(),
        });
    }
    if meta.final_pass
        || distilled.seal_open
        || log
            .last()
            .is_some_and(|s| s.windows >= meta.segment_windows)
    {
        seal(&mut log);
    }
    log
}

/// Seal the open segment, if there is one: a `final` record has none.
fn seal(log: &mut [Segment]) {
    if let Some(open) = log.last_mut() {
        open.state = SegmentState::Sealed;
    }
}

/// The session ended with nothing new since the last pass: the same record,
/// marked final, its open segment sealed, without a model call.
#[must_use]
pub fn finalized(previous: &SessionRecord, now: OffsetDateTime) -> SessionRecord {
    let mut record = previous.clone();
    record.status = SessionStatus::Final;
    record.updated = now;
    seal(&mut record.log);
    record
}
