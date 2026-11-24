//! Text primitives every layer may use: the ranking tokens search and the
//! consistency routine compare (§6.2), the sensitive and finance words the
//! memory rules and stored messages check (§13.8). Pure functions and
//! constants over text; no I/O (STD-02 R5). Each is re-exported from the
//! module that first owned it (scribe-core's `memory` and `capture`). The
//! SHA-256 hashes a cite and a transcript boundary carry are the store's
//! (`scribe_store::digest`).
//!
//! Beside them: the quote check ([`verbatim`]), the claim checks
//! ([`claim`]) and the trigger phrases ([`trigger`]).

pub mod claim;
pub mod trigger;
pub mod verbatim;

use serde::{Deserialize, Serialize};
use std::fmt;

/// Words that carry no meaning for ranking.
const STOPWORDS: &[&str] = &[
    "about", "above", "after", "again", "all", "also", "and", "any", "are", "because", "been",
    "before", "being", "between", "both", "but", "can", "could", "did", "does", "doing", "done",
    "each", "for", "from", "had", "has", "have", "having", "her", "here", "him", "his", "how",
    "into", "its", "just", "let", "like", "made", "make", "may", "more", "most", "much", "must",
    "not", "now", "off", "once", "only", "other", "our", "out", "over", "own", "please", "same",
    "say", "said", "she", "should", "some", "such", "than", "that", "the", "their", "them", "then",
    "there", "these", "they", "this", "those", "through", "too", "under", "until", "very", "was",
    "way", "were", "what", "when", "where", "which", "while", "who", "whom", "why", "will", "with",
    "would", "yes", "you", "your", "yours",
];

/// The ranking tokens of `text` (§6.2): lowercase, split on anything not a
/// letter or digit, stopwords and tokens under three characters dropped,
/// and a light suffix strip (`-ing`, `-ed`, `-es`, `-s`, then a final `-e`,
/// so `merge`, `merges`, `merged` and `merging` are one token).
#[must_use]
pub fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && !STOPWORDS.contains(w))
        .map(stem)
        .collect()
}

fn stem(word: &str) -> String {
    let mut stem = word;
    for suffix in ["ing", "ed", "es", "s"] {
        if let Some(base) = stem.strip_suffix(suffix)
            && base.chars().count() >= 3
            && !(suffix == "s" && base.ends_with('s'))
        {
            stem = base;
            break;
        }
    }
    if stem.chars().count() > 3
        && let Some(base) = stem.strip_suffix('e')
    {
        stem = base;
    }
    stem.to_owned()
}

/// What kind of sensitive event an item records (§13.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SensitiveKind {
    /// A death or a loss.
    Loss,
    /// A health event, never its detail.
    Health,
    /// A relationship event.
    Relationship,
    /// A family event.
    Family,
}

impl SensitiveKind {
    /// The kind's name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loss => "loss",
            Self::Health => "health",
            Self::Relationship => "relationship",
            Self::Family => "family",
        }
    }
}

impl fmt::Display for SensitiveKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Words that mark financial detail; an element holding one is refused
/// (§13.8). Matched on whole words, case aside.
pub const FINANCE_WORDS: &[&str] = &[
    "salary",
    "salaries",
    "paycheck",
    "wage",
    "wages",
    "income",
    "balance",
    "debt",
    "debts",
    "owe",
    "owes",
    "owed",
    "loan",
    "loans",
    "mortgage",
    "iban",
    "net worth",
    "bank account",
    "account number",
    "card number",
    "credit card",
    "routing number",
];

/// Words that mark a sensitive event, with its kind: when one matches, the
/// element is sensitive whatever the model said (§13.8).
pub const SENSITIVE_WORDS: &[(&str, SensitiveKind)] = &[
    ("died", SensitiveKind::Loss),
    ("passed away", SensitiveKind::Loss),
    ("funeral", SensitiveKind::Loss),
    ("death", SensitiveKind::Loss),
    ("miscarriage", SensitiveKind::Loss),
    ("hospital", SensitiveKind::Health),
    ("surgery", SensitiveKind::Health),
    ("diagnosed", SensitiveKind::Health),
    ("diagnosis", SensitiveKind::Health),
    ("cancer", SensitiveKind::Health),
    ("divorce", SensitiveKind::Relationship),
    ("divorced", SensitiveKind::Relationship),
    ("breakup", SensitiveKind::Relationship),
    ("separated", SensitiveKind::Relationship),
    ("pregnant", SensitiveKind::Family),
    ("pregnancy", SensitiveKind::Family),
];

/// Whether `text` holds `phrase` as whole words, case aside.
#[must_use]
pub fn has_phrase(text: &str, phrase: &str) -> bool {
    let words: Vec<String> = text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect();
    let wanted: Vec<&str> = phrase.split(' ').collect();
    !wanted.is_empty()
        && words
            .windows(wanted.len())
            .any(|w| w.iter().zip(&wanted).all(|(a, b)| a == b))
}

/// The kind of the first sensitive word in `text`, if any.
#[must_use]
pub fn sensitive_word(text: &str) -> Option<SensitiveKind> {
    SENSITIVE_WORDS
        .iter()
        .find(|(word, _)| has_phrase(text, word))
        .map(|(_, kind)| *kind)
}

/// Whether `text` holds a listed finance word.
#[must_use]
pub fn finance_word(text: &str) -> bool {
    FINANCE_WORDS.iter().any(|word| has_phrase(text, word))
}
