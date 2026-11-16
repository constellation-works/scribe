//! The one redactor for persisted free text (STD-05 R13, R14;
//! docs/design/scribe/4_decisions.md, "Persisted free text passes one
//! redactor").
//!
//! Before the store writes an entry, a session record or a proposal, every
//! free-text field in [`REDACTED_FIELDS`] passes [`Redactor`], which masks the
//! live values of the configured credential variables
//! ([`crate::Config::secret_names`]) and high-confidence credential shapes
//! (STD-05 R14): an `Authorization: Bearer …` token, `ghp_`/`github_pat_`,
//! `sk-ant-`/`sk-`, `AKIA…` and `xox[bp]-` tokens, and PEM private-key
//! blocks. It matches values and shapes, not words, so a sentence about a
//! token is kept and the token itself is not, and ordinary hyphenated words
//! such as `task-pilot` survive. Identifiers,
//! timestamps and paths are not free text and are left alone; audit lines
//! carry no free text at all. No I/O: the caller reads the environment and
//! hands the values in (STD-02 R3).

use crate::config::Config;
use crate::entry::Entry;
use crate::proposal::Proposal;
use crate::session::{SEGMENT_HEADING_CHARS, SessionRecord};

/// Every free-text field the redactor covers, as `<record>.<field>`. A unit
/// test pins this list against the fields [`Redactor`] actually masks; a new
/// free-text field joins both or the test fails.
pub const REDACTED_FIELDS: &[&str] = &[
    "entry.said",
    "entry.content",
    "entry.retirement_reason",
    "entry.quote",
    "entry.window.text",
    "session.title",
    "session.summary",
    "session.goals",
    "session.next",
    "session.terms.term",
    "session.terms.said",
    "session.log.heading",
    "session.log.text",
    "session.active_change.reason",
    "proposal.reason",
    "proposal.quotes.text",
    "tag.description",
];

/// A value shorter than this is not masked: a one-character "secret" would
/// shred every record, and no real credential is that short.
pub const MIN_SECRET_CHARS: usize = 8;

/// Masks configured secret values in free text.
#[derive(Clone, Default)]
pub struct Redactor {
    /// `(variable name, value)`, longest value first, so a secret that
    /// contains another is masked whole.
    secrets: Vec<(String, String)>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the values themselves (STD-05 R13).
        let names: Vec<&str> = self.secrets.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("Redactor").field("names", &names).finish()
    }
}

impl Redactor {
    /// A redactor for `(name, value)` pairs. Empty and short values are
    /// skipped (see [`MIN_SECRET_CHARS`]).
    pub fn new(secrets: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut secrets: Vec<(String, String)> = secrets
            .into_iter()
            .filter(|(_, value)| value.chars().count() >= MIN_SECRET_CHARS)
            .collect();
        secrets.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
        secrets.dedup_by(|a, b| a.1 == b.1);
        Self { secrets }
    }

    /// A redactor for the variables `config` names, their values looked up
    /// with `lookup` (the process environment, in the CLI).
    pub fn from_config(config: &Config, lookup: impl Fn(&str) -> Option<String>) -> Self {
        Self::new(
            config
                .secret_names()
                .into_iter()
                .filter_map(|name| lookup(name).map(|value| (name.to_owned(), value))),
        )
    }

    /// `text` with every configured value replaced by `[redacted:<NAME>]`
    /// and every high-confidence credential shape replaced by a labelled
    /// marker such as `[redacted:github-token]`.
    #[must_use]
    pub fn mask(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (name, value) in &self.secrets {
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), &format!("[redacted:{name}]"));
            }
        }
        mask_shapes(&out)
    }

    fn mask_all<'a>(&self, fields: impl IntoIterator<Item = (&'static str, &'a mut String)>) {
        for (_, text) in fields {
            *text = self.mask(text);
        }
    }

    /// Mask an entry's free text.
    pub fn entry(&self, entry: &mut Entry) {
        self.mask_all(entry_fields(entry));
    }

    /// Mask a store tag's description.
    pub fn tag(&self, tag: &mut crate::StoreTag) {
        self.mask_all(tag_fields(tag));
    }

    /// Mask a session record's free text. A segment heading a marker made
    /// longer than [`SEGMENT_HEADING_CHARS`] is cut back to it, so the
    /// record still reads back.
    pub fn session(&self, record: &mut SessionRecord) {
        self.mask_all(session_fields(record));
        for segment in &mut record.log {
            if segment.heading.chars().count() > SEGMENT_HEADING_CHARS {
                segment.heading = segment
                    .heading
                    .chars()
                    .take(SEGMENT_HEADING_CHARS)
                    .collect::<String>()
                    .trim_end()
                    .to_owned();
            }
        }
    }

    /// Mask a proposal's free text.
    pub fn proposal(&self, proposal: &mut Proposal) {
        self.mask_all(proposal_fields(proposal));
    }
}

/// The `[redact] env` names in `config` whose value `lookup` cannot supply
/// (absent or empty). The model credentials are not included: they are
/// alternatives, so one may legitimately be unset. A caller that persists
/// free text refuses to write while this is non-empty, rather than store
/// text it was told to mask but cannot (STD-05 R13).
#[must_use]
pub fn unresolved_redact_env(
    config: &Config,
    lookup: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    config
        .redact_env
        .iter()
        .filter(|name| lookup(name).is_none_or(|value| value.is_empty()))
        .cloned()
        .collect()
}

/// The shortest tail after a credential prefix that counts as a credential.
/// Shorter look-alikes (a word, a truncated example) are left alone.
const MIN_SHAPE_TAIL: usize = 8;

/// Whether `byte` may sit inside a credential token of the given alphabet.
fn is_shape_char(byte: u8, alphabet: &[u8]) -> bool {
    byte.is_ascii_alphanumeric() || alphabet.contains(&byte)
}

/// A match at `i` starts a new token: it is not the tail of a longer word
/// or identifier (STD-05 R14: never mask part of an ordinary identifier).
fn at_token_start(bytes: &[u8], i: usize) -> bool {
    i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || matches!(bytes[i - 1], b'_' | b'-'))
}

/// The end of the token starting at `i`, over `alphabet` plus alphanumerics.
fn token_end(bytes: &[u8], i: usize, alphabet: &[u8]) -> usize {
    let mut end = i;
    while end < bytes.len() && is_shape_char(bytes[end], alphabet) {
        end += 1;
    }
    end
}

/// A PEM private-key block starting at `i`: its end (the whole block, or
/// the rest of the text when the end marker is missing, failing closed).
fn pem_block_end(text: &str, i: usize) -> Option<usize> {
    let rest = text.get(i..)?;
    if !rest.starts_with("-----BEGIN ") {
        return None;
    }
    let header_end = rest.find('\n').unwrap_or(rest.len());
    if !rest[..header_end].contains("PRIVATE KEY-----") {
        return None;
    }
    let close = "PRIVATE KEY-----";
    Some(
        rest.find("-----END ")
            .and_then(|start| {
                rest[start..]
                    .find(close)
                    .map(|at| i + start + at + close.len())
            })
            .unwrap_or(text.len()),
    )
}

/// An `Authorization: Bearer <token>` header starting at `i`
/// (case-insensitive): the token's `(start, end)`, header kept as written.
fn bearer_token_span(text: &str, i: usize) -> Option<(usize, usize)> {
    let rest = text.get(i..)?;
    let lower = rest.to_ascii_lowercase();
    let after_scheme = lower.strip_prefix("authorization:")?;
    let after_scheme = after_scheme.trim_start_matches([' ', '\t']);
    let after_bearer = after_scheme.strip_prefix("bearer")?;
    if !after_bearer.starts_with([' ', '\t']) {
        return None;
    }
    let token = after_bearer.trim_start_matches([' ', '\t']);
    let token_start = i + (rest.len() - token.len());
    let bytes = text.as_bytes();
    let end = token_end(bytes, token_start, b"._~+/=-");
    (end - token_start >= MIN_SHAPE_TAIL).then_some((token_start, end))
}

/// A prefixed credential starting at `i`: its `(end, replacement)`.
fn prefixed_shape(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    // Longest prefixes first, so `sk-ant-…` is not claimed by `sk-`.
    const PREFIXES: &[(&[u8], &[u8], &str)] = &[
        (b"github_pat_", b"_", "[redacted:github-token]"),
        (b"ghp_", b"", "[redacted:github-token]"),
        (b"sk-ant-", b"-_", "[redacted:api-key]"),
        (b"sk-", b"-_", "[redacted:api-key]"),
        (b"xoxb-", b"-", "[redacted:slack-token]"),
        (b"xoxp-", b"-", "[redacted:slack-token]"),
    ];
    for (prefix, alphabet, replacement) in PREFIXES {
        if bytes[i..].starts_with(prefix) {
            let tail_start = i + prefix.len();
            let end = token_end(bytes, tail_start, alphabet);
            if end - tail_start >= MIN_SHAPE_TAIL {
                return Some((end, replacement));
            }
        }
    }
    // An AWS access key id: `AKIA` plus exactly 16 upper-case/digit chars.
    if bytes[i..].starts_with(b"AKIA") {
        let end = i + 4 + 16;
        if end <= bytes.len()
            && bytes[i + 4..end]
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            && (end == bytes.len() || !bytes[end].is_ascii_alphanumeric())
        {
            return Some((end, "[redacted:aws-access-key]"));
        }
    }
    None
}

/// `text` with every high-confidence credential shape masked.
fn mask_shapes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if at_token_start(bytes, i) {
            if let Some(end) = pem_block_end(text, i) {
                out.push_str("[redacted:private-key]");
                i = end;
                continue;
            }
            if let Some((start, end)) = bearer_token_span(text, i) {
                out.push_str(&text[i..start]);
                out.push_str("[redacted:bearer]");
                i = end;
                continue;
            }
            if let Some((end, replacement)) = prefixed_shape(bytes, i) {
                out.push_str(replacement);
                i = end;
                continue;
            }
        }
        let ch_len = text[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// The free-text fields of an entry, named as in [`REDACTED_FIELDS`].
pub(super) fn entry_fields(entry: &mut Entry) -> Vec<(&'static str, &mut String)> {
    let mut fields = vec![
        ("entry.said", &mut entry.said),
        ("entry.content", &mut entry.content),
    ];
    if let Some(reason) = &mut entry.retirement_reason {
        fields.push(("entry.retirement_reason", reason));
    }
    if let Some(quote) = &mut entry.quote {
        fields.push(("entry.quote", quote));
    }
    if let Some(window) = &mut entry.window {
        for turn in &mut window.turns {
            fields.push(("entry.window.text", &mut turn.text));
        }
    }
    fields
}

/// The free-text fields of a session record; each term and each segment
/// is two fields.
pub(super) fn session_fields(record: &mut SessionRecord) -> Vec<(&'static str, &mut String)> {
    let mut fields = vec![
        ("session.title", &mut record.title),
        ("session.summary", &mut record.summary),
        ("session.goals", &mut record.goals),
        ("session.next", &mut record.next),
    ];
    for term in &mut record.terms {
        fields.push(("session.terms.term", &mut term.term));
        fields.push(("session.terms.said", &mut term.said));
    }
    for segment in &mut record.log {
        fields.push(("session.log.heading", &mut segment.heading));
        fields.push(("session.log.text", &mut segment.text));
    }
    if let Some(change) = &mut record.active_change {
        fields.push(("session.active_change.reason", &mut change.reason));
    }
    fields
}

/// The free-text fields of a proposal; each quote is one field.
pub(super) fn proposal_fields(proposal: &mut Proposal) -> Vec<(&'static str, &mut String)> {
    let mut fields = vec![("proposal.reason", &mut proposal.reason)];
    fields.extend(
        proposal
            .quotes
            .iter_mut()
            .map(|q| ("proposal.quotes.text", &mut q.text)),
    );
    fields
}

/// Store tag free text.
pub(super) fn tag_fields(tag: &mut crate::StoreTag) -> Vec<(&'static str, &mut String)> {
    vec![("tag.description", &mut tag.description)]
}
