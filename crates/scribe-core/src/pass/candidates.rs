//! Claim admission, provenance and duplicate policy (2_design.md §5.3,
//! §11.2). No I/O; the store repeats the trigger match and the duplicate
//! checks under its lock before reserving an entry id.

use crate::models::{EntryCandidateAnswer, FiringTurn, PassMessage, SessionRequest, TagCriteria};
use crate::tag::{TagDef, TagTable};
use crate::{
    Admitted, Cite, Entry, EntryCandidate, Proposal, SessionRecord, Speaker, TagName, transcript,
    trigger, verbatim,
};
use std::collections::BTreeSet;

/// Maximum candidates per session in one pass invocation, across all windows.
pub const MAX_CANDIDATES: usize = 3;
/// Maximum open candidate proposals across the whole store.
pub const MAX_OPEN_CANDIDATES: usize = 20;

/// The capture tags' firing turns in a window and their criteria
/// (2_design.md §11.2): every Daniel turn of `messages` matched against
/// every tag's triggers, model-free. Empty when none fires, and the caller
/// then requests zero claims.
#[must_use]
pub fn capture_request(
    tags: &TagTable,
    messages: &[PassMessage],
) -> (Vec<FiringTurn>, Vec<TagCriteria>) {
    let mut turns = Vec::new();
    let mut rules: Vec<TagCriteria> = Vec::new();
    for message in messages.iter().filter(|m| m.role == Speaker::User) {
        for tag in tags.tags() {
            let Some(fired) = fire(tag, &message.text) else {
                continue;
            };
            turns.push(FiringTurn {
                message: message.uuid.clone(),
                tag: tag.name.to_string(),
                trigger: fired.trigger,
                matched: fired.matched,
            });
            if !rules.iter().any(|r| r.tag == tag.name.as_str()) {
                rules.push(TagCriteria {
                    tag: tag.name.to_string(),
                    criteria: tag.capture.criteria.as_ref().map(|c| c.value.clone()),
                    not: tag.capture.not.as_ref().map(|n| n.value.clone()),
                });
            }
        }
    }
    (turns, rules)
}

/// The first trigger of `tag` that fires in Daniel's own words `own`
/// (fenced, indented, quoted and `#scribe` lines left out).
fn fire(tag: &TagDef, own: &str) -> Option<trigger::Fired> {
    let triggers = tag.capture.pass_triggers()?;
    trigger::first_fired(triggers, &own_lines(own))
}

/// What admits a claim under `tag` from Daniel's own words `own`: the
/// first trigger of the tag that fires there, with its matched words and
/// the tag's `criteria` in force; `None` when the tag has no pass triggers
/// or none fires. The store calls it again under its lock.
#[must_use]
pub(crate) fn admission(tags: &TagTable, tag: &TagName, own: &str) -> Option<Admitted> {
    let def = tags.get(tag)?;
    let fired = fire(def, own)?;
    Some(Admitted {
        trigger: fired.trigger,
        matched: fired.matched,
        criteria: def.capture.criteria.as_ref().map(|c| c.value.clone()),
    })
}

pub(super) fn check(
    answers: Vec<EntryCandidateAnswer>,
    request: &SessionRequest,
) -> Vec<EntryCandidateAnswer> {
    let mut checked: Vec<EntryCandidateAnswer> = Vec::new();
    for answer in answers {
        if checked.len() >= request.candidate_limit.min(MAX_CANDIDATES) {
            break;
        }
        // Only a turn the request named, under the tag named for it, and
        // only Daniel's own span of it (the fenced/quoted exclusion kept).
        let named = request
            .candidate_turns
            .iter()
            .any(|turn| turn.message == answer.message && turn.tag == answer.tag);
        let fits = named
            && request.messages.iter().any(|message| {
                message.uuid == answer.message
                    && message.role == Speaker::User
                    && own_span(&message.text, &answer.quote)
            });
        if answer.quote.chars().count() > 400
            || answer.message.chars().count() > 128
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

/// `text` with fenced, indented and blockquoted material and marked
/// instructions replaced by a NUL barrier each, so neither a quote nor a
/// trigger match can be spliced across an excluded line.
fn own_lines(text: &str) -> String {
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
    own
}

/// Whether `quote` is Daniel's own span of `text`: verbatim, and outside
/// fenced, indented and blockquoted material and marked instructions.
pub(crate) fn own_span(text: &str, quote: &str) -> bool {
    verbatim::contains(text, quote) && text.contains(quote) && own_lines(text).contains(quote)
}

/// A checked claim on its way to the store: the candidate and Daniel's own
/// words of the turn it cites, against which the store repeats the trigger
/// match under its lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassClaim {
    /// The candidate, with the admission the pass computed.
    pub candidate: EntryCandidate,
    /// Daniel's own words of the cited turn (pasted blocks left out).
    pub turn: String,
}

/// Derive durable provenance from the original transcript, never model
/// metadata: only Daniel's turns, and only when a trigger of the claim's tag
/// fires in that turn's original text.
#[must_use]
pub fn candidate_drafts(
    checked: &[EntryCandidateAnswer],
    messages: &[transcript::Message],
    record: &SessionRecord,
    tags: &TagTable,
) -> Vec<PassClaim> {
    checked
        .iter()
        .filter_map(|answer| {
            let message = messages.iter().find(|m| m.uuid == answer.message)?;
            if message.role != Speaker::User {
                return None;
            }
            let raw = message.text();
            // The original text check also rejects a span joined across pasted data
            // or a window's truncation marker.
            let turn = transcript::daniel_turn(message)?;
            if !own_span(&turn.own, &answer.quote) || !raw.contains(&answer.quote) {
                return None;
            }
            let tag: TagName = answer.tag.parse().ok()?;
            let admitted = admission(tags, &tag, &turn.own)?;
            let start = raw.find(&answer.quote)?;
            let line =
                u32::try_from(raw[..start].bytes().filter(|b| *b == b'\n').count() + 1).ok()?;
            Some(PassClaim {
                candidate: EntryCandidate {
                    tag,
                    role: Speaker::User,
                    quote: answer.quote.clone(),
                    evidence: None,
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
                    admitted: Some(admitted),
                },
                turn: turn.own,
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
    let statements = entries.iter().map(|e| e.content.as_str()).chain(
        proposals
            .iter()
            .filter_map(|p| p.candidate.as_ref())
            .map(|c| c.quote.as_str()),
    );
    let mut ranked: Vec<_> = statements
        .map(|text| (words(text).intersection(&query).count(), text))
        .filter(|(score, _)| *score > 0)
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    ranked
        .into_iter()
        .take(15)
        .map(|(_, text)| text.chars().take(400).collect())
        .collect()
}
