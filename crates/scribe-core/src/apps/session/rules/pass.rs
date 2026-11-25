//! A session pass: from a switched-on session's transcript to its distilled
//! session record (docs/design/scribe/2_design.md §5.3).
//!
//! The session app reads the transcript and the Orbit job runs the model.
//! These rules build the request, check the answer and construct the record:
//!
//! 1. [`conversation`](scribe_tools::window::conversation): the
//!    transcript's messages from the first one (the whole session, not only
//!    those since `on_since`), synthetic turns
//!    (`<task-notification>`, command echoes, `/scribe` itself) dropped and
//!    pasted blocks set apart from Daniel's own words;
//! 2. [`window_with_limits`](scribe_tools::window::window_with_limits): the
//!    next consecutive messages after the record's `scribed_through`, within
//!    the configured message and character limits;
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
//! outcome, at most [`MAX_COLLECTED`] per session per invocation.
//!
//! The pass writes no other entries, whatever the transcript holds. No I/O
//! (STD-02 R5).

use crate::models::{
    LinkedRecord, PassMessage, PreviousRecord, SESSION_BUDGET, SegmentHeading, SessionAnswer,
    SessionRequest,
};
use crate::record::{Label, Project};
use crate::session::{
    Segment, SegmentOrigin, SegmentState, SessionId, SessionLink, SessionRecord, SessionStatus,
};
use scribe_tools::window::Window;
use std::num::NonZeroU32;
use time::OffsetDateTime;

pub use crate::apps::proposal::admission;
use crate::apps::proposal::candidates;
pub use crate::apps::proposal::{
    MAX_CANDIDATES, MAX_COLLECTED, candidate_drafts, candidate_exclusions, capture_request,
    write_exclusions,
};
pub use crate::claim::PassClaim;
pub use crate::claim::{
    candidate_says, claim_covers, covering_claim, covering_entry, entry_says, own_span,
};
pub use crate::config::{
    MAX_OPEN_CANDIDATES, SEGMENT_WINDOWS, WINDOW_CHARS, WINDOW_MESSAGES, WindowLimits,
};
pub use scribe_tools::session_distill::{
    BUDGETS, CheckedSegment, Distilled, Invalid, SAID_CHARS, SEGMENT_CHARS, TERM_CHARS,
    compression_request, merge_terms,
};

/// The most sessions one `session pass` run processes (2_design.md §5.2).
pub const MAX_PER_RUN: usize = 5;

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
            open: r.open_segment().map(Into::into),
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
        memory: false,
        memory_limit: 0,
        memory_now: Vec::new(),
        memory_pending: Vec::new(),
        budgets: BUDGETS,
        final_pass,
        correction: None,
    }
}

/// Check the shared answer contract and the capture rules for this pass.
pub fn check(
    answer: SessionAnswer,
    conversation: &[PassMessage],
    request: &SessionRequest,
) -> Result<Distilled, Invalid> {
    let mut checked = scribe_tools::session_distill::check(answer, conversation, request)?;
    checked.candidates = candidates::check(checked.candidates, request);
    Ok(checked)
}

/// Repair the final retry, then apply the same capture rules.
pub fn repair(
    answer: SessionAnswer,
    conversation: &[PassMessage],
    request: &SessionRequest,
) -> Result<Distilled, Invalid> {
    let mut checked = scribe_tools::session_distill::repair(answer, conversation, request)?;
    checked.candidates = candidates::check(checked.candidates, request);
    Ok(checked)
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
