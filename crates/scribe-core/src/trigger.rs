//! Trigger phrases: the model-free first gate of a tag's capture rules
//! (docs/design/scribe/2_design.md §11.2).
//!
//! A trigger is a phrase of words matched case-insensitively on word
//! boundaries. Inside a phrase, `*` stands for zero to
//! [`MAX_WILDCARD_WORDS`] words, and the whole match lies within one
//! sentence: a sentence ends at `.`, `!`, `?`, a line break or a NUL (the
//! barrier the pass puts where it left a line out). Pure values and no I/O
//! (STD-02 R5).

/// The most words one `*` stands for.
pub const MAX_WILDCARD_WORDS: usize = 4;

/// The longest trigger phrase, in characters.
pub const MAX_TRIGGER_CHARS: usize = 120;

/// The most triggers one tag may list.
pub const MAX_TRIGGERS: usize = 32;

/// The longest `matched` text kept as provenance, in characters.
pub const MATCHED_CHARS: usize = 120;

/// A trigger that fired in a text: the phrase as configured and the words of
/// the text it matched, verbatim (at most [`MATCHED_CHARS`] characters).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fired {
    /// The trigger phrase, as configured.
    pub trigger: String,
    /// The text's words it matched, verbatim.
    pub matched: String,
}

/// Why a trigger phrase was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TriggerError {
    /// No word in it.
    #[error("trigger '{0}' has no word to match")]
    Empty(String),
    /// Longer than [`MAX_TRIGGER_CHARS`].
    #[error("trigger '{0}' is longer than {MAX_TRIGGER_CHARS} characters")]
    TooLong(String),
    /// A `*` at its start or end, or two in a row.
    #[error("trigger '{0}': '*' stands between two words, once")]
    Wildcard(String),
    /// A `*` glued to a word, or a sentence end inside the phrase.
    #[error(
        "trigger '{0}' must be words separated by spaces, with '*' as a word of its own and no '.', '!' or '?'"
    )]
    Shape(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pat {
    Word(String),
    Gap,
}

/// Check one trigger phrase as config gives it.
pub fn check(phrase: &str) -> Result<(), TriggerError> {
    parse(phrase).map(|_| ())
}

fn parse(phrase: &str) -> Result<Vec<Pat>, TriggerError> {
    if phrase.chars().count() > MAX_TRIGGER_CHARS {
        return Err(TriggerError::TooLong(phrase.to_owned()));
    }
    if phrase.chars().any(is_sentence_end) {
        return Err(TriggerError::Shape(phrase.to_owned()));
    }
    let mut pats = Vec::new();
    for piece in phrase.split_whitespace() {
        if piece == "*" {
            pats.push(Pat::Gap);
            continue;
        }
        if piece.contains('*') {
            return Err(TriggerError::Shape(phrase.to_owned()));
        }
        pats.extend(words(piece).into_iter().map(|w| Pat::Word(w.norm)));
    }
    if !pats.iter().any(|p| matches!(p, Pat::Word(_))) {
        return Err(TriggerError::Empty(phrase.to_owned()));
    }
    let gap_at_edge =
        matches!(pats.first(), Some(Pat::Gap)) || matches!(pats.last(), Some(Pat::Gap));
    let gaps_in_a_row = pats.windows(2).any(|w| matches!(w, [Pat::Gap, Pat::Gap]));
    if gap_at_edge || gaps_in_a_row {
        return Err(TriggerError::Wildcard(phrase.to_owned()));
    }
    Ok(pats)
}

fn is_sentence_end(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '\n' | '\r' | '\0')
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}')
}

/// One word of a text: its byte range, its sentence and its folded form.
struct Word {
    start: usize,
    end: usize,
    sentence: usize,
    norm: String,
}

/// The words of `text`: runs of letters and digits, with an apostrophe kept
/// only between two of them (`here's`, `don't`).
fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut sentence = 0usize;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0usize;
    while let Some(&(at, c)) = chars.get(i) {
        if is_sentence_end(c) {
            sentence += 1;
            i += 1;
            continue;
        }
        if !c.is_alphanumeric() {
            i += 1;
            continue;
        }
        let start = at;
        let mut end = at + c.len_utf8();
        let mut norm: String = c.to_lowercase().collect();
        i += 1;
        while let Some(&(at, c)) = chars.get(i) {
            if c.is_alphanumeric() {
                norm.extend(c.to_lowercase());
                end = at + c.len_utf8();
                i += 1;
            } else if is_apostrophe(c)
                && chars
                    .get(i + 1)
                    .is_some_and(|(_, next)| next.is_alphanumeric())
            {
                norm.push('\'');
                end = at + c.len_utf8();
                i += 1;
            } else {
                break;
            }
        }
        out.push(Word {
            start,
            end,
            sentence,
            norm,
        });
    }
    out
}

/// Where `pats[p..]` matches `words[w..]` within `sentence`: the index of
/// the last word matched, when the rest of the phrase matches.
fn match_from(pats: &[Pat], words: &[Word], sentence: usize, w: usize) -> Option<usize> {
    let Some((first, rest)) = pats.split_first() else {
        return w.checked_sub(1);
    };
    match first {
        Pat::Word(norm) => {
            let word = words.get(w)?;
            (word.sentence == sentence && &word.norm == norm)
                .then(|| match_from(rest, words, sentence, w + 1))
                .flatten()
        }
        Pat::Gap => (0..=MAX_WILDCARD_WORDS).find_map(|skip| {
            // Every skipped word must lie in the same sentence.
            let skipped_ok =
                (w..w + skip).all(|i| words.get(i).is_some_and(|x| x.sentence == sentence));
            skipped_ok
                .then(|| match_from(rest, words, sentence, w + skip))
                .flatten()
        }),
    }
}

/// The words of `text` that `phrase` matches first, verbatim and cut to
/// [`MATCHED_CHARS`]; `None` when it does not occur or is not a valid
/// trigger.
#[must_use]
pub fn find(phrase: &str, text: &str) -> Option<String> {
    let pats = parse(phrase).ok()?;
    let words = words(text);
    (0..words.len()).find_map(|start| {
        let sentence = words.get(start)?.sentence;
        let last = match_from(&pats, &words, sentence, start)?;
        let from = words.get(start)?.start;
        let to = words.get(last)?.end;
        text.get(from..to)
            .map(|m| m.chars().take(MATCHED_CHARS).collect())
    })
}

/// The first of `triggers`, in list order, that fires in `text`.
#[must_use]
pub fn first_fired(triggers: &[String], text: &str) -> Option<Fired> {
    triggers.iter().find_map(|trigger| {
        find(trigger, text).map(|matched| Fired {
            trigger: trigger.clone(),
            matched,
        })
    })
}
