//! Claim admission, provenance and duplicate policy (2_design.md §5.3,
//! §11.2). No I/O; the store repeats the trigger match and the duplicate
//! checks under its lock before reserving an entry id.

use crate::models::{EntryCandidateAnswer, FiringTurn, PassMessage, SessionRequest, TagCriteria};
use crate::tag::{TagDef, TagTable};
use crate::{
    Admitted, Cite, Entry, EntryCandidate, EntryStatus, Project, Proposal, STATEMENT_CHARS,
    SUBJECT_CHARS, SessionRecord, Speaker, TagName, transcript, trigger, verbatim,
};
use std::collections::BTreeSet;
use time::OffsetDateTime;

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
        // A claim names what it is about and states it so it stands
        // alone (2_design.md §11.1): an empty subject or statement, or one
        // past its bound, opens nothing.
        let statement = answer.statement.trim().to_owned();
        let subject = answer.subject.trim().to_owned();
        if answer.quote.chars().count() > 400
            || answer.message.chars().count() > 128
            || statement.is_empty()
            || statement.chars().count() > STATEMENT_CHARS
            || subject.is_empty()
            || subject.chars().count() > SUBJECT_CHARS
            || !fits
            || checked.iter().any(|prior| {
                claim_covers(
                    &[prior.quote.as_str(), prior.statement.as_str()],
                    &[answer.quote.as_str(), statement.as_str()],
                )
            })
        {
            continue;
        }
        let mut answer = answer;
        answer.statement = statement;
        answer.subject = subject;
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
            if answer.statement.trim().is_empty() || answer.subject.trim().is_empty() {
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
                    statement: Some(answer.statement.trim().to_owned())
                        .filter(|text| !text.is_empty()),
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
                turn: turn.own,
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

/// Whether `entry` covered `statement` at `at`.
///
/// Live capture uses [`covering_entry`] and ignores anything that is not
/// active now, so a line typed after an entry is retired is still written.
/// Stop recovery also asks this, so a line covered when it was typed is
/// not captured after that entry is retired or superseded.
///
/// An entry that is still active covers `at` when it already existed
/// (`created <= at`). A retired or superseded entry covers `at` when it
/// existed and left `active` at or after `at` (`inactive_since`, which a
/// later metadata write does not move), and its content is still the
/// statement. Stored times are whole seconds while transcript times are
/// not, so both sides are compared in whole seconds: a tie inside that
/// second counts as still active, since the order inside the second is not
/// recorded and recapturing a line that may have been covered is the worse
/// miss. A forgotten entry's content is blank, so it cannot answer.
#[must_use]
pub(crate) fn covers_turn(
    entry: &Entry,
    statement: &str,
    tag: &TagName,
    project: &Project,
    at: OffsetDateTime,
) -> bool {
    let at = whole_second(at);
    if &entry.tag != tag || &entry.project != project || whole_second(entry.created) > at {
        return false;
    }
    let active_then = match entry.status {
        EntryStatus::Active => true,
        EntryStatus::Retired | EntryStatus::Superseded => entry
            .inactive_since
            .is_some_and(|since| whole_second(since) >= at),
        EntryStatus::Forgotten => false,
    };
    active_then && covers_statement(&entry.content, statement)
}

/// `at` without its fraction of a second.
fn whole_second(at: OffsetDateTime) -> OffsetDateTime {
    at.replace_nanosecond(0).unwrap_or(at)
}

/// The oldest active entry of `tag` and `project` whose content already
/// says `statement`, if any. Retired, superseded and forgotten entries do
/// not cover a new line. Comparison is [`covers_statement`]. A turn those
/// entries were still active for is [`covers_turn`].
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
        .filter(|entry| covers_statement(&entry.content, statement))
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

/// What a candidate says: Daniel's quote and, when the pass gave one,
/// its statement.
pub(crate) fn candidate_says(candidate: &EntryCandidate) -> Vec<&str> {
    let mut texts = vec![candidate.quote.as_str()];
    if let Some(statement) = &candidate.statement {
        texts.push(statement);
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
    let statements = entries.iter().map(|e| e.content.as_str()).chain(
        proposals
            .iter()
            .filter_map(|p| p.candidate.as_ref())
            .map(|c| c.statement.as_deref().unwrap_or(&c.quote)),
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
