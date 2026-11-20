//! Findings the session pass writes itself (docs/design/scribe/2_design.md
//! §4.7): an assistant turn's measured or verified result, copied verbatim
//! from the original transcript. The model only names the message and the
//! first and last words of the span; the content is never model text. No I/O;
//! the store repeats the duplicate, redaction and supersede checks under its
//! lock.

use super::candidates::own_span;
use crate::models::{FindingAnswer, SessionRequest};
use crate::record::{Label, Project};
use crate::{Cite, EntryId, SessionRecord, Speaker, transcript};

/// The most findings one session's pass writes in one invocation, across
/// all its windows (apart from the [`super::MAX_CANDIDATES`] candidates).
pub const MAX_PASS_FINDINGS: usize = 5;

/// The longest span a finding copies, in characters; a longer span is cut at
/// a paragraph or table-row boundary and marked with [`FINDING_CUT_MARK`].
pub const FINDING_SPAN_CHARS: usize = 4_000;

/// The longest `start` or `end` words the model names, in characters.
pub const FINDING_BOUND_CHARS: usize = 80;

/// What ends a finding cut to [`FINDING_SPAN_CHARS`], after a blank line.
pub const FINDING_CUT_MARK: &str = "[… cut at 4,000 characters]";

/// A finding the pass will write: the span copied from the named assistant
/// turn, with its provenance derived from the transcript and the record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingDraft {
    /// The assistant's own words, byte for byte, then [`FINDING_CUT_MARK`]
    /// after a blank line when `cut`.
    pub content: String,
    /// Whether the span was longer than [`FINDING_SPAN_CHARS`] and was cut.
    pub cut: bool,
    /// What the turn names to check the result against, else the session
    /// segment holding the turn, `session:<id>#<n>`.
    pub evidence: String,
    /// The earlier finding this one revises, as the model named it; the
    /// store applies it only to an active agent-authored finding of the
    /// same project.
    pub supersedes: Option<EntryId>,
    /// The assistant turn; its `at` is the finding's as-of date.
    pub cite: Cite,
    /// The session's project.
    pub project: Project,
    /// The transcript's provider.
    pub provider: Label,
    /// The host that read the transcript.
    pub host: Label,
}

/// Keep the findings that name an assistant turn of this window within the
/// bounds, at most `request.finding_limit` (never above
/// [`MAX_PASS_FINDINGS`]). Whether the span exists is checked against the
/// original transcript by [`finding_drafts`].
pub(super) fn check(answers: Vec<FindingAnswer>, request: &SessionRequest) -> Vec<FindingAnswer> {
    let limit = request.finding_limit.min(MAX_PASS_FINDINGS);
    let mut checked: Vec<FindingAnswer> = Vec::new();
    for mut answer in answers {
        if checked.len() >= limit {
            break;
        }
        answer.start = answer.start.trim().to_owned();
        answer.end = answer.end.trim().to_owned();
        answer.evidence = answer
            .evidence
            .map(|e| e.trim().to_owned())
            .filter(|e| !e.is_empty());
        answer.supersedes = answer
            .supersedes
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        let bound = |s: &str| !s.is_empty() && s.chars().count() <= FINDING_BOUND_CHARS;
        let assistant_turn = request
            .messages
            .iter()
            .any(|m| m.uuid == answer.message && m.role == Speaker::Assistant);
        if !assistant_turn
            || answer.message.chars().count() > 128
            || !bound(&answer.start)
            || !bound(&answer.end)
            || answer
                .evidence
                .as_deref()
                .is_some_and(|e| e.chars().count() > 512)
            || answer
                .supersedes
                .as_deref()
                .is_some_and(|s| s.chars().count() > 128)
            || checked.iter().any(|prior| {
                prior.message == answer.message
                    && prior.start == answer.start
                    && prior.end == answer.end
            })
        {
            continue;
        }
        checked.push(answer);
    }
    checked
}

/// The byte range of the span from the first occurrence of `start` to the
/// first `end` that closes after it, inside `text`.
fn locate(text: &str, start: &str, end: &str) -> Option<(usize, usize)> {
    let from = text.find(start)?;
    let min_close = from + start.len();
    let close = text[from..]
        .match_indices(end)
        .map(|(i, m)| from + i + m.len())
        .find(|close| *close >= min_close)?;
    Some((from, close))
}

/// Cut `span` to at most [`FINDING_SPAN_CHARS`] characters at the last
/// paragraph boundary (a blank line) or table-row boundary (the end of a
/// `|` line) inside the bound. `None` when there is no such boundary.
fn cut(span: &str) -> Option<&str> {
    let limit = span
        .char_indices()
        .nth(FINDING_SPAN_CHARS)
        .map_or(span.len(), |(i, _)| i);
    let head = &span[..limit];
    let mut best = None;
    let mut line_start = 0usize;
    for (i, _) in head.match_indices('\n') {
        let line = &head[line_start..i];
        let blank = line.trim().is_empty();
        let row = line.trim_start().starts_with('|');
        if (blank || row) && !head[..line_start].trim().is_empty() {
            best = Some(if blank { line_start } else { i });
        }
        line_start = i + 1;
    }
    best.map(|end| head[..end].trim_end())
        .filter(|kept| !kept.is_empty())
}

/// The segment of `record` that holds message `uuid`: the last one whose
/// first message is at or before it in transcript order.
fn segment_of(record: &SessionRecord, messages: &[transcript::Message], uuid: &str) -> Option<u32> {
    let at = messages.iter().position(|m| m.uuid == uuid)?;
    record
        .log
        .iter()
        .filter(|s| {
            s.first_message
                .as_ref()
                .and_then(|first| messages.iter().position(|m| &m.uuid == first))
                .is_some_and(|first| first <= at)
        })
        .map(|s| s.n.get())
        .next_back()
}

/// Copy each checked finding's span from the original transcript turn.
/// A span whose words do not occur in the named assistant message, or that
/// takes in fenced code, an indented or quoted block or a `#scribe` line,
/// writes nothing; so does a span over [`FINDING_SPAN_CHARS`] with no
/// paragraph or table-row boundary to cut at. Tables are allowed.
#[must_use]
pub fn finding_drafts(
    checked: &[FindingAnswer],
    messages: &[transcript::Message],
    record: &SessionRecord,
) -> Vec<FindingDraft> {
    checked
        .iter()
        .filter_map(|answer| {
            let message = messages
                .iter()
                .find(|m| m.uuid == answer.message && m.role == Speaker::Assistant)?;
            let raw = message.text();
            let (from, close) = locate(&raw, &answer.start, &answer.end)?;
            let span = &raw[from..close];
            if span.trim().is_empty() || !own_span(&raw, span) {
                return None;
            }
            let (content, was_cut) = if span.chars().count() > FINDING_SPAN_CHARS {
                (format!("{}\n\n{FINDING_CUT_MARK}", cut(span)?), true)
            } else {
                (span.to_owned(), false)
            };
            let line =
                u32::try_from(raw[..from].bytes().filter(|b| *b == b'\n').count() + 1).ok()?;
            let evidence = answer.evidence.clone().unwrap_or_else(|| {
                match segment_of(record, messages, &message.uuid) {
                    Some(n) => format!("session:{}#{n}", record.id),
                    None => format!("session:{}", record.id),
                }
            });
            Some(FindingDraft {
                content,
                cut: was_cut,
                evidence,
                supersedes: answer
                    .supersedes
                    .as_deref()
                    .and_then(|s| s.parse::<EntryId>().ok()),
                cite: Cite {
                    session: record.id.clone(),
                    message: Some(message.uuid.clone()),
                    at: message.at,
                    prompt_sha256: crate::capture::prompt_sha256(&raw),
                    line,
                    cwd: record.cwd.clone(),
                },
                project: record.project.clone(),
                provider: record.provider.clone(),
                host: record.host.clone(),
            })
        })
        .collect()
}
