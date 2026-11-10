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
//! 4. [`check`]: the answer's budgets and shape, refused whole when wrong
//!    (the caller keeps the previous record), and every term kept only when
//!    its `said` is a verbatim span of one of Daniel's own turns;
//! 5. [`record`]: the new record, `scribed_through` advanced.
//!
//! Until S9 gives the model the log itself, a pass writes the header
//! (title, summary, goals, next) and puts the answer's `outcome` in the
//! record's open segment: it opens one (`origin: pass`) when there is none,
//! extends its message range and `windows` otherwise, and seals it at the
//! final pass. Sealed segments, the upgraded one included, are never
//! touched.
//!
//! A pass writes no tag entries, whatever the transcript holds. No I/O
//! (STD-02 R5).

use crate::control;
use crate::entry::Speaker;
use crate::models::{
    Budgets, CUT_MARK, LinkedRecord, PassMessage, PreviousRecord, SESSION_BUDGET, SessionAnswer,
    SessionRequest,
};
use crate::record::{Label, Project, Related, Topic, Topics};
use crate::session::{
    HEADER_SECTIONS, LOG_SECTION, SEGMENT_HEADING_CHARS, Segment, SegmentOrigin, SegmentState,
    SessionId, SessionLink, SessionRecord, SessionStatus, Term,
};
use crate::transcript::Message;
use crate::verbatim;
use std::num::NonZeroU32;
use time::OffsetDateTime;

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

/// The character budgets of an answer: the title, the header's sections,
/// and `outcome`, the open segment's text. S9 replaces these with the
/// segment budgets of 2_design.md §5.5.
pub const BUDGETS: Budgets = Budgets {
    title: 120,
    summary: 1500,
    goals: 600,
    outcome: 2500,
    next: 1000,
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
/// sessions this one links to ([`linked_context`]).
#[must_use]
pub fn request(
    model: &str,
    previous: Option<&SessionRecord>,
    linked: Vec<LinkedRecord>,
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
            outcome: r.open_segment().map(|s| s.text.clone()).unwrap_or_default(),
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
    /// The open segment's text.
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
/// record parser would read as a section or segment heading.
fn section(name: &'static str, text: &str, max: usize) -> Result<String, Invalid> {
    let text = text.trim().to_owned();
    within(name, &text, max)?;
    let heading = text.lines().any(|line| {
        line.starts_with("### ")
            || line
                .strip_prefix("## ")
                .is_some_and(|h| h == LOG_SECTION || h == "Outcome" || HEADER_SECTIONS.contains(&h))
    });
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
}

/// The record a pass writes. A previous record keeps its `created`,
/// `started`, `cwd`, `project`, `provider` and `host` (the file stays where
/// it is), its activity fields and catalog marker, its sealed segments and
/// its terms, the new ones appended; `passes` counts up and
/// `scribed_through` advances to the last message the pass read. The
/// answer's `outcome` becomes the open segment's text ([`log_after`]).
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

/// The log after one window: the previous record's segments, sealed ones
/// unchanged. The open segment, when there is one, takes the answer's
/// `outcome` as its text, the title as its heading, the answer's topics and
/// references, and the window's last message and time; its `windows`
/// counts up. Without one, a non-empty `outcome` opens segment `n + 1`
/// over the window. The final pass seals the last segment.
fn log_after(
    previous: Option<&SessionRecord>,
    distilled: &Distilled,
    meta: &PassMeta,
) -> Vec<Segment> {
    let mut log = previous.map_or_else(Vec::new, |p| p.log.clone());
    let heading = segment_heading(&distilled.title);
    let through = meta.through.clone();
    match log.last_mut().filter(|s| s.state == SegmentState::Open) {
        Some(open) => {
            open.heading = heading;
            open.text.clone_from(&distilled.outcome);
            open.topics = distilled.topics.clone();
            open.related.clone_from(&distilled.related);
            if through.is_some() {
                open.last_message = through;
                open.to = meta.to;
            }
            open.windows = open.windows.saturating_add(1);
        }
        None if !distilled.outcome.is_empty() => {
            let n = u32::try_from(log.len())
                .unwrap_or(u32::MAX)
                .saturating_add(1);
            log.push(Segment {
                n: NonZeroU32::new(n).unwrap_or(NonZeroU32::MAX),
                heading,
                first_message: meta.first_message.clone().or_else(|| through.clone()),
                last_message: through,
                from: meta.from,
                to: meta.to,
                state: SegmentState::Open,
                windows: NonZeroU32::MIN,
                origin: SegmentOrigin::Pass,
                topics: distilled.topics.clone(),
                related: distilled.related.clone(),
                text: distilled.outcome.clone(),
            });
        }
        None => {}
    }
    if meta.final_pass {
        seal(&mut log);
    }
    log
}

/// A segment heading from a title: one line, at most
/// [`SEGMENT_HEADING_CHARS`] characters; `Session` when the title is empty.
fn segment_heading(title: &str) -> String {
    let flat = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = flat.chars().take(SEGMENT_HEADING_CHARS).collect();
    let cut = cut.trim_end().to_owned();
    if cut.is_empty() {
        String::from("Session")
    } else {
        cut
    }
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
