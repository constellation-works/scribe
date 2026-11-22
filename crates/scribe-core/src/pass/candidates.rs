//! Claim admission, provenance and duplicate policy (2_design.md §5.3,
//! §11.2). No I/O; the store repeats the trigger match and the duplicate
//! checks under its lock before reserving an entry id.

use crate::models::{EntryCandidateAnswer, FiringTurn, PassMessage, SessionRequest, TagCriteria};
use crate::tag::{TagDef, TagTable};
use crate::{
    Admitted, Cite, Entry, EntryCandidate, EntryStatus, Outcome, OutcomeState, Project, Proposal,
    SUBJECT_CHARS, SessionRecord, Speaker, TagName, transcript, trigger, verbatim,
};
use std::collections::BTreeSet;

/// Maximum candidates per session in one pass invocation, across all windows.
pub const MAX_CANDIDATES: usize = 3;
/// Maximum open candidate proposals across the whole store.
pub const MAX_OPEN_CANDIDATES: usize = 20;
/// Maximum entries the pass collects itself under one `write` tag
/// (2_design.md §11.6, §15.2) per session in one pass invocation, across
/// all windows, apart from [`MAX_CANDIDATES`].
pub const MAX_COLLECTED: usize = 3;
/// Longest parking words an idea's outcome keeps, in characters.
pub const OUTCOME_REASON_CHARS: usize = 300;
/// Longest task id an idea's outcome names, in characters.
const OUTCOME_TASK_CHARS: usize = 64;
/// Most existing entries sent as write exclusions, and the characters of
/// each ([`write_exclusions`]).
const WRITE_EXCLUSIONS: usize = 10;
const WRITE_EXCLUSION_CHARS: usize = 400;

/// The catchers' firing turns in a window and their criteria
/// (2_design.md §11.2, §15.2): every turn of `messages` matched against the
/// triggers of every catcher whose `from` reads its speaker, model-free.
/// Empty when none fires, and the caller then requests zero claims.
#[must_use]
pub fn capture_request(
    tags: &TagTable,
    messages: &[PassMessage],
) -> (Vec<FiringTurn>, Vec<TagCriteria>) {
    let mut turns = Vec::new();
    let mut rules: Vec<TagCriteria> = Vec::new();
    for message in messages {
        // The pass never reads handoff text into claims, even under an
        // assistant or both catcher (2_design.md §16).
        if message.role == Speaker::Assistant && crate::handoff::is_handoff(&message.text) {
            continue;
        }
        for tag in tags.tags() {
            if !reads(tag, message.role) {
                continue;
            }
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
                    writes: tag.capture.writes(),
                });
            }
        }
    }
    (turns, rules)
}

/// Whether `tag` is a catcher whose `from` reads turns by `speaker`.
pub(crate) fn reads(tag: &TagDef, speaker: Speaker) -> bool {
    tag.capture.from().is_some_and(|from| from.reads(speaker))
}

/// The first trigger of `tag` that fires in a turn's own words `own`
/// (fenced, indented, quoted and `#scribe` lines left out). A marked turn
/// fires nothing ([`is_marked`]), so it supplies no candidate and no
/// written idea, here and under the store lock.
pub(crate) fn fire(tag: &TagDef, own: &str) -> Option<trigger::Fired> {
    let triggers = tag.capture.pass_triggers()?;
    if is_marked(own) {
        return None;
    }
    trigger::first_fired(triggers, &own_lines(own))
}

/// Whether Daniel marked the turn whose own words are `own` (2_design.md
/// §5.3, §14.7): a `#scribe` marker inside a sentence, as
/// [`crate::annotation::marks`] finds it (refused markers included, and
/// since §14.6 every line-start form but a settling command), or a line
/// that opens with the marker, which adds the form-4 settling lines. Fenced, quoted
/// and pasted lines hold neither, as `capture::plain_lines` leaves them out.
/// The pass skips the whole turn; it never reads `marks.json`.
pub(crate) fn is_marked(own: &str) -> bool {
    !crate::annotation::marks(own).is_empty()
        || crate::capture::plain_lines(own).iter().any(|(_, line)| {
            !line.starts_with("    ")
                && !line.starts_with('\t')
                && line.trim_start().starts_with(crate::capture::MARKER)
        })
}

/// What admits a claim under `tag` from the own words `own` of a turn by
/// `speaker`: the first trigger of the tag that fires there, with its
/// matched words and the tag's `criteria` in force; `None` when the tag is
/// no catcher, does not read that speaker, or none fires. The store calls
/// it again under its lock.
#[must_use]
pub(crate) fn admission(
    tags: &TagTable,
    tag: &TagName,
    speaker: Speaker,
    own: &str,
) -> Option<Admitted> {
    if speaker == Speaker::Assistant && crate::handoff::is_handoff(own) {
        return None;
    }
    let def = tags.get(tag)?;
    if !reads(def, speaker) {
        return None;
    }
    let fired = fire(def, own)?;
    Some(Admitted {
        trigger: fired.trigger,
        matched: fired.matched,
        // Stored and shown, so `{user}` reads "the user", never the name.
        criteria: def
            .capture
            .criteria
            .as_ref()
            .map(|c| crate::voice::UserName::default().fill(&c.value)),
    })
}

pub(super) fn check(
    answers: Vec<EntryCandidateAnswer>,
    request: &SessionRequest,
) -> Vec<EntryCandidateAnswer> {
    let writes = |tag: &str| {
        request
            .candidate_rules
            .iter()
            .any(|rule| rule.tag == tag && rule.writes)
    };
    let mut checked: Vec<EntryCandidateAnswer> = Vec::new();
    let mut proposed = 0_usize;
    let mut written: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for answer in answers {
        // Proposals and collected entries have separate slots (§11.6), the
        // collected ones per tag (§15.2).
        let write = writes(&answer.tag);
        let (taken, limit) = if write {
            (
                written.get(&answer.tag).copied().unwrap_or(0),
                request.write_limit.min(MAX_COLLECTED),
            )
        } else {
            (proposed, request.candidate_limit.min(MAX_CANDIDATES))
        };
        if taken >= limit {
            continue;
        }
        // Only a turn the request named, under the tag named for it (so a
        // turn by a speaker the tag's `from` reads), and only the turn's
        // own span (the fenced/quoted exclusion kept).
        let named = request
            .candidate_turns
            .iter()
            .any(|turn| turn.message == answer.message && turn.tag == answer.tag);
        let fits = named
            && request.messages.iter().any(|message| {
                message.uuid == answer.message && own_span(&message.text, &answer.quote)
            });
        // A claim names what it is about and gives the context it can be
        // settled from (2_design.md §15.3): an empty subject, a context
        // under two sentences, or either past its bound, opens nothing.
        let context = answer.context.trim().to_owned();
        let subject = answer.subject.trim().to_owned();
        if answer.quote.chars().count() > 400
            || answer.message.chars().count() > 128
            || !crate::context_stands_alone(&context)
            || subject.is_empty()
            || subject.chars().count() > SUBJECT_CHARS
            || !fits
            || (write && !outcome_in_bounds(&answer))
            || checked.iter().any(|prior| {
                claim_covers(
                    &[prior.quote.as_str(), prior.context.as_str()],
                    &[answer.quote.as_str(), context.as_str()],
                )
            })
        {
            continue;
        }
        let mut answer = answer;
        answer.context = context;
        answer.subject = subject;
        let tag = answer.tag.clone();
        checked.push(answer);
        if write {
            *written.entry(tag).or_default() += 1;
        } else {
            proposed += 1;
        }
    }
    checked
}

/// Whether a written claim's outcome fields are a known state within
/// their bounds; the verbatim checks come later, against the transcript.
fn outcome_in_bounds(answer: &EntryCandidateAnswer) -> bool {
    matches!(
        answer.outcome.as_deref(),
        None | Some("open" | "parked" | "task")
    ) && answer
        .outcome_said
        .as_ref()
        .is_none_or(|said| said.chars().count() <= OUTCOME_REASON_CHARS)
        && answer
            .outcome_message
            .as_ref()
            .is_none_or(|id| id.chars().count() <= 128)
        && answer
            .task
            .as_ref()
            .is_none_or(|task| task.chars().count() <= OUTCOME_TASK_CHARS)
}

/// `text` with fenced, indented and blockquoted material and marked
/// instructions replaced by a NUL barrier each, so neither a quote nor a
/// trigger match can be spliced across an excluded line.
pub(crate) fn own_lines(text: &str) -> String {
    own_lines_where(text, |_| false)
}

/// [`own_lines`], but a line that opens with `#scribe` is kept when
/// `keep` says so: an annotation's check keeps a line-start marker, whose
/// words are Daniel's (2_design.md §14.6).
fn own_lines_where(text: &str, keep: impl Fn(&str) -> bool) -> String {
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
            || (trimmed.starts_with("#scribe") && !keep(trimmed.trim_end()))
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

/// [`own_span`] for an annotation (§14.3): a line-start `#scribe` line is a
/// marker whose words are Daniel's own (§14.6), so a quote may come from
/// it; a settling command's line is still left out.
pub(crate) fn own_marked_span(text: &str, quote: &str) -> bool {
    verbatim::contains(text, quote)
        && text.contains(quote)
        && own_lines_where(text, |line| !crate::capture::settles(line)).contains(quote)
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
    /// Under a `write` tag, the idea's checked outcome (`open` or `parked`;
    /// an idea that became a task in the session is not drafted); `None`
    /// under a `propose` tag.
    pub outcome: Option<Outcome>,
    /// For a `parked` outcome, Daniel's own words of the turn its reason is
    /// in, against which the store repeats the verbatim check.
    pub outcome_turn: Option<String>,
}

/// A written claim's outcome checked against the transcript: `None` when
/// the idea became a task in the session (its id appears in the
/// conversation, at least three characters), so no entry is written. A
/// `parked` reason that is not a
/// verbatim span of Daniel's own words in the named turn, or a `task` id
/// the conversation never writes, leaves the idea `open`.
fn checked_outcome(
    answer: &EntryCandidateAnswer,
    messages: &[transcript::Message],
) -> Option<(Outcome, Option<String>)> {
    match answer.outcome.as_deref() {
        Some("task") => {
            let task = answer.task.as_deref().map(str::trim).unwrap_or_default();
            let became = is_task_id(task)
                && messages
                    .iter()
                    .any(|message| names_task_id(&message.text(), task));
            if became {
                return None;
            }
            Some((Outcome::open(), None))
        }
        Some("parked") => {
            let parked = answer
                .outcome_said
                .as_deref()
                .zip(answer.outcome_message.as_deref())
                .and_then(|(said, id)| {
                    let message = messages
                        .iter()
                        .find(|m| m.uuid == id && m.role == Speaker::User)?;
                    let turn = transcript::user_turn(message)?;
                    (!said.trim().is_empty()
                        && own_span(&turn.own, said)
                        && message.text().contains(said))
                    .then(|| {
                        (
                            Outcome {
                                state: OutcomeState::Parked,
                                reason: Some(said.to_owned()),
                                task: None,
                            },
                            Some(turn.own),
                        )
                    })
                });
            Some(parked.unwrap_or((Outcome::open(), None)))
        }
        _ => Some((Outcome::open(), None)),
    }
}

/// Whether `task` is shaped like a task id: at least 3 characters, one
/// token, holding a digit (`ORB-4242`, `#112`, `owner/repo#7`). An ordinary
/// word such as `task` or `the` is not one, so it never stands for a task.
fn is_task_id(task: &str) -> bool {
    task.chars().count() >= 3
        && !task.chars().any(char::is_whitespace)
        && task.chars().any(|ch| ch.is_ascii_digit())
}

/// Whether `text` writes `task` as a whole token: no id character touches
/// either end of the match, so `ORB-42` is not found in `ORB-4242`.
fn names_task_id(text: &str, task: &str) -> bool {
    let joins = |ch: char| ch.is_alphanumeric() || ch == '-' || ch == '_';
    text.match_indices(task).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + task.len()..].chars().next();
        !before.is_some_and(joins) && !after.is_some_and(joins)
    })
}

/// Derive durable provenance from the original transcript, never model
/// metadata: only turns by a speaker the claim's tag reads, and only when a
/// trigger of the tag fires in that turn's own text. An assistant claim
/// keeps its speaker (`role`), so it is never presented as the user's.
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
            let raw = message.text();
            // The original text check also rejects a span joined across pasted data
            // or a window's truncation marker.
            let own = match message.role {
                Speaker::User => transcript::user_turn(message)?.own,
                Speaker::Assistant => raw.trim().to_owned(),
            };
            if !own_span(&own, &answer.quote) || !raw.contains(&answer.quote) {
                return None;
            }
            if !crate::context_stands_alone(&answer.context) || answer.subject.trim().is_empty() {
                return None;
            }
            let tag: TagName = answer.tag.parse().ok()?;
            let admitted = admission(tags, &tag, message.role, &own)?;
            let (outcome, outcome_turn) = if tags.get(&tag)?.capture.writes() {
                let (outcome, outcome_turn) = checked_outcome(answer, messages)?;
                (Some(outcome), outcome_turn)
            } else {
                (None, None)
            };
            let start = raw.find(&answer.quote)?;
            let line =
                u32::try_from(raw[..start].bytes().filter(|b| *b == b'\n').count() + 1).ok()?;
            Some(PassClaim {
                candidate: EntryCandidate {
                    tag,
                    role: message.role,
                    quote: answer.quote.clone(),
                    context: Some(answer.context.trim().to_owned()).filter(|text| !text.is_empty()),
                    statement: None,
                    subject: Some(answer.subject.trim().to_owned()).filter(|text| !text.is_empty()),
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
                turn: own,
                outcome,
                outcome_turn,
            })
        })
        .collect()
}

/// How many tokens a new statement needs before an existing statement can
/// cover it by containing those tokens. Shorter statements match only when
/// the token sequences are equal.
const MIN_COVER_TOKENS: usize = 4;

/// Negation and exception words. A difference in how often any of these
/// appears means the new statement is not the same claim. `dont` is the
/// apostrophe-less spelling of `don't`; the tokenizer keeps an interior
/// apostrophe, so both forms occur. The same pairing covers `can't`,
/// `won't`, `shouldn't`, `doesn't`, `didn't` and `isn't`.
///
const MARKERS: [&str; 23] = [
    "not",
    "never",
    "no",
    "don't",
    "dont",
    "can't",
    "cant",
    "cannot",
    "won't",
    "wont",
    "shouldn't",
    "shouldnt",
    "doesn't",
    "doesnt",
    "didn't",
    "didnt",
    "isn't",
    "isnt",
    "avoid",
    "except",
    "unless",
    "only",
    "without",
];

pub(crate) fn is_marker(token: &str) -> bool {
    MARKERS.contains(&token)
}

/// Ordered tokens of a statement. One normalizer for the duplicate check
/// and for [`words`]: Unicode normalization, trimmed, lowercased, with curly
/// apostrophes folded to `'`. A token is a run of alphanumeric characters;
/// an apostrophe between alphanumerics stays, so `don't` is one token.
pub(crate) fn statement_tokens(text: &str) -> Vec<String> {
    let normalized = verbatim::normalize(text);
    let folded = normalized.replace(['\u{2018}', '\u{2019}'], "'");
    let lower = folded.trim().to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if ch.is_alphanumeric() {
            current.push(ch);
            index += 1;
            continue;
        }
        if ch == '\''
            && !current.is_empty()
            && chars.get(index + 1).is_some_and(|ch| ch.is_alphanumeric())
        {
            current.push('\'');
            index += 1;
            continue;
        }
        if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
        index += 1;
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// The token set [`candidate_exclusions`] ranks with. Derived from
/// [`statement_tokens`], not a second splitter.
fn words(text: &str) -> BTreeSet<String> {
    statement_tokens(text).into_iter().collect()
}

fn marker_counts(tokens: &[String]) -> [usize; MARKERS.len()] {
    let mut counts = [0; MARKERS.len()];
    for token in tokens {
        if let Some(slot) = MARKERS.iter().position(|marker| *marker == token) {
            counts[slot] += 1;
        }
    }
    counts
}

/// Whether `existing` already says `new`.
///
/// Token sequences are compared in order, on token boundaries. Equal
/// sequences match at any length, so case and punctuation differences
/// collapse. Otherwise `existing` covers `new` only when `new` adds
/// nothing: it is a contiguous window of `existing`, it has at least
/// [`MIN_COVER_TOKENS`] tokens, and the two sides use the same negation
/// and exception words. A longer refinement is never covered by a shorter
/// statement, a reordered claim does not match, and a polarity or
/// exception change does not match. Empty text matches nothing.
///
/// This is the one lexical duplicate predicate. Entry creation applies it
/// through [`covering_entry`] (active, same tag, same project). Candidate
/// opening applies it directly to every readable entry and every prior
/// candidate, in any state and across tags and projects, because a
/// suggestion of an already-recorded claim is still a duplicate there.
/// Meaning-level paraphrases are not this check.
#[must_use]
pub(crate) fn covers_statement(existing: &str, new: &str) -> bool {
    let existing = statement_tokens(existing);
    let new = statement_tokens(new);
    if existing.is_empty() || new.is_empty() {
        return false;
    }
    if existing == new {
        return true;
    }
    if marker_counts(&existing) != marker_counts(&new) {
        return false;
    }
    if new.len() < MIN_COVER_TOKENS {
        return false;
    }
    existing
        .windows(new.len())
        .any(|window| window == new.as_slice())
}

/// The oldest active entry of `tag` and `project` that already says
/// `statement` ([`entry_says`]: its content or, for an approved pass
/// statement, Daniel's quote), if any. Retired, superseded and forgotten entries do
/// not cover a new line. Comparison is [`covers_statement`].
#[must_use]
pub(crate) fn covering_entry<'a>(
    entries: &'a [Entry],
    statement: &str,
    tag: &TagName,
    project: &Project,
) -> Option<&'a Entry> {
    entries
        .iter()
        .filter(|entry| {
            entry.status == EntryStatus::Active && &entry.tag == tag && &entry.project == project
        })
        .filter(|entry| claim_covers(&entry_says(entry), &[statement]))
        .min_by(|left, right| {
            left.created
                .cmp(&right.created)
                .then_with(|| left.id.cmp(&right.id))
        })
}

/// Whether any of the `existing` texts already says any of the `new`
/// ones, by [`covers_statement`]. A pass claim says two things, Daniel's
/// quote and scribe's statement of it, and an approved one keeps both
/// (content and `quote`), so the duplicate checks compare every pair
/// (2_design.md §11.1).
#[must_use]
pub(crate) fn claim_covers(existing: &[&str], new: &[&str]) -> bool {
    existing
        .iter()
        .any(|old| new.iter().any(|text| covers_statement(old, text)))
}

/// What an entry says, for the claim duplicate checks: its content and, for
/// an approved pass statement, Daniel's quote beside it.
pub(crate) fn entry_says(entry: &Entry) -> Vec<&str> {
    let mut texts = vec![entry.content.as_str()];
    if entry.stated_by.is_some()
        && let Some(quote) = &entry.quote
    {
        texts.push(quote);
    }
    texts
}

/// What a candidate says: the speaker's quote and, when the pass gave
/// one, its context (or an older claim's statement).
pub(crate) fn candidate_says(candidate: &EntryCandidate) -> Vec<&str> {
    let mut texts = vec![candidate.quote.as_str()];
    if let Some(stated) = candidate.stated() {
        texts.push(stated);
    }
    texts
}

/// The oldest active entry of the candidate's tag and project that already
/// says its quote or its statement ([`claim_covers`]), if any: the shared
/// check approval runs before it writes the candidate's entry.
#[must_use]
pub(crate) fn covering_claim<'a>(
    entries: &'a [Entry],
    candidate: &EntryCandidate,
) -> Option<&'a Entry> {
    let new = candidate_says(candidate);
    entries
        .iter()
        .filter(|entry| {
            entry.status == EntryStatus::Active
                && entry.tag == candidate.tag
                && entry.project == candidate.project
        })
        .filter(|entry| claim_covers(&entry_says(entry), &new))
        .min_by(|left, right| {
            left.created
                .cmp(&right.created)
                .then_with(|| left.id.cmp(&right.id))
        })
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
    let statements = entries
        .iter()
        .filter(|e| e.tag.as_str() != crate::handoff::HANDOFF_TAG)
        .map(|e| e.content.as_str())
        .chain(
            proposals
                .iter()
                .filter_map(|p| p.candidate.as_ref())
                .map(|c| c.stated().unwrap_or(&c.quote)),
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

/// The active entries of the request's `write` tags in `project`, for the
/// model to leave out (2_design.md §11.6): the lexical check cannot match a
/// short typed idea against the long raw paragraph it summarizes, so the
/// model sees what is already recorded. Each is its content and, for a
/// stated entry, Daniel's words; ranked by words shared with the window,
/// then newest first; at most [`WRITE_EXCLUSIONS`] of
/// [`WRITE_EXCLUSION_CHARS`] characters each (4,000 in all). Empty when no
/// write tag fires or no write slot is left.
#[must_use]
pub fn write_exclusions(
    entries: &[Entry],
    project: &Project,
    request: &SessionRequest,
) -> Vec<String> {
    if request.write_limit == 0 {
        return Vec::new();
    }
    let written: Vec<&str> = request
        .candidate_rules
        .iter()
        .filter(|rule| rule.writes)
        .map(|rule| rule.tag.as_str())
        .filter(|tag| request.candidate_turns.iter().any(|turn| turn.tag == *tag))
        .collect();
    if written.is_empty() {
        return Vec::new();
    }
    let query = words(
        &request
            .messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    );
    let mut ranked: Vec<(usize, &Entry, String)> = entries
        .iter()
        .filter(|entry| {
            entry.status == EntryStatus::Active
                && &entry.project == project
                && written.contains(&entry.tag.as_str())
        })
        .map(|entry| {
            let text = match crate::ask::stated_quote(entry) {
                Some(quote) => match crate::ask::speaker_of(entry) {
                    Speaker::User => format!("{} (the user's words: {quote:?})", entry.content),
                    Speaker::Assistant => {
                        format!("{} (the assistant's words: {quote:?})", entry.content)
                    }
                },
                None => entry.content.clone(),
            };
            (words(&text).intersection(&query).count(), entry, text)
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.created.cmp(&a.1.created))
            .then_with(|| a.1.id.cmp(&b.1.id))
    });
    ranked
        .into_iter()
        .take(WRITE_EXCLUSIONS)
        .map(|(_, _, text)| text.chars().take(WRITE_EXCLUSION_CHARS).collect())
        .collect()
}
