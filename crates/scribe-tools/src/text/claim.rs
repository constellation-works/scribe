//! Lexical provenance and duplicate checks for claims.

use super::verbatim;
use crate::entry::{Entry, EntryStatus, Outcome, TagName};
use crate::proposal::EntryCandidate;
use crate::record::Project;

/// `text` with fenced, indented and blockquoted material and marked
/// instructions replaced by a NUL barrier each, so neither a quote nor a
/// trigger match can be spliced across an excluded line.
pub fn own_lines(text: &str) -> String {
    own_lines_where(text, |_| false)
}

/// [`own_lines`], but a line that opens with `#scribe` is kept when
/// `keep` says so: an annotation's check keeps a line-start marker, whose
/// words are Daniel's (2_design.md §14.6).
pub(crate) fn own_lines_where(text: &str, keep: impl Fn(&str) -> bool) -> String {
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
pub fn own_span(text: &str, quote: &str) -> bool {
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
    /// Under a `write` tag, the idea's checked outcome (`open` or `parked`;
    /// an idea that became a task in the session is not drafted); `None`
    /// under a `propose` tag.
    pub outcome: Option<Outcome>,
    /// For a `parked` outcome, Daniel's own words of the turn its reason is
    /// in, against which the store repeats the verbatim check.
    pub outcome_turn: Option<String>,
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

/// Whether `token` is a negation or exception marker.
#[must_use]
pub fn is_marker(token: &str) -> bool {
    MARKERS.contains(&token)
}

/// Ordered tokens of a statement. One normalizer for the duplicate check
/// and for exclusion ranking: Unicode normalization, trimmed, lowercased, with curly
/// apostrophes folded to `'`. A token is a run of alphanumeric characters;
/// an apostrophe between alphanumerics stays, so `don't` is one token.
pub fn statement_tokens(text: &str) -> Vec<String> {
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
pub fn covering_entry<'a>(
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
pub fn claim_covers(existing: &[&str], new: &[&str]) -> bool {
    existing
        .iter()
        .any(|old| new.iter().any(|text| covers_statement(old, text)))
}

/// What an entry says, for the claim duplicate checks: its content and, for
/// an approved pass statement, Daniel's quote beside it.
pub fn entry_says(entry: &Entry) -> Vec<&str> {
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
pub fn candidate_says(candidate: &EntryCandidate) -> Vec<&str> {
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
pub fn covering_claim<'a>(entries: &'a [Entry], candidate: &EntryCandidate) -> Option<&'a Entry> {
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
