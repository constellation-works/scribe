//! The one quote check (docs/design/scribe/2_design.md §6.4).
//!
//! Every place scribe accepts words as someone's — a referent span, a
//! proposal's quotes, a claim in an answer — runs them through
//! [`contains`]. Both sides are normalized the same way, so a quote survives
//! line wrapping and Unicode composition differences but nothing else: no
//! case folding and no punctuation stripping. No I/O (STD-02 R5).

use unicode_normalization::UnicodeNormalization;

/// The shortest quote accepted, in characters after normalization. A shorter
/// one ("yes", "ok") would match almost any source.
pub const MIN_QUOTE_CHARS: usize = 8;

/// `text` in the form the check compares: NFC, `\r\n` as `\n`, and every run
/// of whitespace (line breaks included) as one space.
#[must_use]
pub fn normalize(text: &str) -> String {
    let composed: String = text.replace("\r\n", "\n").nfc().collect();
    let mut out = String::with_capacity(composed.len());
    let mut in_space = false;
    for c in composed.chars() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// Whether `quote` is a verbatim span of `source`: after [`normalize`] and
/// trimming the quote's ends, it is at least [`MIN_QUOTE_CHARS`] characters
/// and a substring of the normalized source.
///
/// ```
/// use scribe_tools::text::verbatim;
///
/// let source = "squash-merge agent PRs;\nkeep merge commits for releases";
/// assert!(verbatim::contains(source, "agent PRs; keep merge"));
/// assert!(!verbatim::contains(source, "Agent PRs"), "no case folding");
/// assert!(!verbatim::contains(source, "PRs"), "too short to mean anything");
/// ```
#[must_use]
pub fn contains(source: &str, quote: &str) -> bool {
    let quote = normalize(quote);
    let quote = quote.trim_matches(' ');
    quote.chars().count() >= MIN_QUOTE_CHARS && normalize(source).contains(quote)
}
