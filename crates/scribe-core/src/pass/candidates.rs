//! Candidate provenance and duplicate policy. No I/O; the store repeats
//! duplicate checks under its lock before reserving an entry id.

use crate::models::{EntryCandidateAnswer, SessionRequest};
use crate::{Cite, Entry, EntryCandidate, Proposal, SessionRecord, Speaker, transcript, verbatim};
use std::collections::BTreeSet;

/// Maximum candidates per session in one pass invocation, across all windows.
pub const MAX_CANDIDATES: usize = 3;
/// Maximum open candidate proposals across the whole store.
pub const MAX_OPEN_CANDIDATES: usize = 20;

pub(super) fn check(
    answers: Vec<EntryCandidateAnswer>,
    request: &SessionRequest,
) -> Vec<EntryCandidateAnswer> {
    let mut checked: Vec<EntryCandidateAnswer> = Vec::new();
    for answer in answers {
        if checked.len() >= request.candidate_limit.min(MAX_CANDIDATES) {
            break;
        }
        // The named turn decides whose words these are. Daniel's own span
        // keeps the fenced/quoted exclusion and may use a memory tag; an
        // assistant turn's own text (never a tool payload: transcripts
        // carry tool calls and results outside `text`) may use any tag
        // offered, under the same cap.
        let fits = request.messages.iter().any(|message| {
            message.uuid == answer.message
                && match message.role {
                    Speaker::User => {
                        request.candidate_memory_tags.contains(&answer.tag)
                            && own_span(&message.text, &answer.quote)
                    }
                    Speaker::Assistant => {
                        request.candidate_tags.contains(&answer.tag)
                            && own_span(&message.text, &answer.quote)
                    }
                }
        });
        if answer.quote.chars().count() > 400
            || answer.message.chars().count() > 128
            || answer
                .evidence
                .as_deref()
                .is_some_and(|e| e.trim().is_empty() || e.chars().count() > 512)
            || !fits
            || checked
                .iter()
                .any(|prior| same_statement(&prior.quote, &answer.quote))
        {
            continue;
        }
        checked.push(answer);
    }
    checked
}

/// Exclude fenced, indented and blockquoted material and marked instructions.
/// Barriers prevent a quote from being spliced across an excluded line.
pub(super) fn own_span(text: &str, quote: &str) -> bool {
    if !verbatim::contains(text, quote) || !text.contains(quote) {
        return false;
    }
    let mut fence: Option<(char, usize)> = None;
    let mut own = String::new();
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'));
        let marker = marker.and_then(|c| {
            let n = trimmed.chars().take_while(|m| *m == c).count();
            (n >= 3).then_some((c, n))
        });
        if let Some((mark, len)) = fence {
            if marker.is_some_and(|(m, n)| m == mark && n >= len) {
                fence = None;
            }
            own.push('\0');
        } else if let Some(marker) = marker {
            fence = Some(marker);
            own.push('\0');
        } else if trimmed.starts_with('>')
            || trimmed.starts_with("#scribe")
            || line.starts_with("    ")
            || line.starts_with('\t')
        {
            own.push('\0');
        } else {
            own.push_str(line);
        }
    }
    own.contains(quote)
}

/// Derive durable provenance from the original transcript, never model metadata.
#[must_use]
pub fn candidate_drafts(
    checked: &[EntryCandidateAnswer],
    messages: &[transcript::Message],
    record: &SessionRecord,
) -> Vec<EntryCandidate> {
    checked
        .iter()
        .filter_map(|answer| {
            let message = messages.iter().find(|m| m.uuid == answer.message)?;
            let raw = message.text();
            // The original text check also rejects a span joined across pasted data
            // or a window's truncation marker. An assistant turn's text holds
            // its own words only; tool payloads never reach `text()`.
            let role = match message.role {
                Speaker::User => {
                    let turn = transcript::daniel_turn(message)?;
                    if !own_span(&turn.own, &answer.quote) {
                        return None;
                    }
                    Speaker::User
                }
                Speaker::Assistant => {
                    if !own_span(&raw, &answer.quote) {
                        return None;
                    }
                    Speaker::Assistant
                }
            };
            if !raw.contains(&answer.quote) {
                return None;
            }
            let start = raw.find(&answer.quote)?;
            let line =
                u32::try_from(raw[..start].bytes().filter(|b| *b == b'\n').count() + 1).ok()?;
            Some(EntryCandidate {
                tag: answer.tag.parse().ok()?,
                role,
                quote: answer.quote.clone(),
                evidence: answer
                    .evidence
                    .as_deref()
                    .map(str::trim)
                    .filter(|e| !e.is_empty())
                    .map(ToOwned::to_owned),
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

fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Conservative lexical duplicate check: normalized containment, equal word
/// sets, or at least 80% Jaccard overlap. Meaning-level deduplication is also
/// requested from the model using the relevant existing statements.
#[must_use]
pub(crate) fn same_statement(a: &str, b: &str) -> bool {
    let a = verbatim::normalize(a).trim().to_lowercase();
    let b = verbatim::normalize(b).trim().to_lowercase();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a.contains(&b) || b.contains(&a) {
        return true;
    }
    let a = words(&a);
    let b = words(&b);
    a == b
        || (a.len().min(b.len()) >= 4 && a.intersection(&b).count() * 5 >= a.union(&b).count() * 4)
}

/// Relevant existing statements, at most 6,000 value characters (400 each).
/// All entries remain available to the deterministic store check, uncapped.
#[must_use]
pub fn candidate_exclusions(
    entries: &[Entry],
    proposals: &[Proposal],
    request: &SessionRequest,
) -> Vec<String> {
    let query = words(
        &request
            .messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    );
    // A recorded statement carries its id, so a revised finding can name
    // the one it supersedes; a proposed one has no entry yet.
    let statements = entries
        .iter()
        .map(|e| (Some(e.id.to_string()), e.content.as_str()))
        .chain(
            proposals
                .iter()
                .filter_map(|p| p.candidate.as_ref())
                .map(|c| (None, c.quote.as_str())),
        );
    let mut ranked: Vec<_> = statements
        .map(|(id, text)| (words(text).intersection(&query).count(), id, text))
        .filter(|(score, _, _)| *score > 0)
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.cmp(b.2)));
    ranked
        .into_iter()
        .take(15)
        .map(|(_, id, text)| {
            let text: String = text.chars().take(400).collect();
            match id {
                Some(id) => format!("[{id}] {text}"),
                None => text,
            }
        })
        .collect()
}
