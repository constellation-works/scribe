//! The one redactor for persisted free text (STD-05 R13, R14;
//! docs/design/scribe/4_decisions.md, "Persisted free text passes one
//! redactor").
//!
//! Before the store writes an entry, a session record or a proposal, every
//! free-text field in [`REDACTED_FIELDS`] passes [`Redactor`], which masks the
//! live values of the configured credential variables
//! ([`crate::Config::secret_names`]). It matches values, not words, so a
//! sentence about a token is kept and the token itself is not. Identifiers,
//! timestamps and paths are not free text and are left alone; audit lines
//! carry no free text at all. No I/O: the caller reads the environment and
//! hands the values in (STD-02 R3).

use crate::config::Config;
use crate::entry::Entry;
use crate::proposal::Proposal;
use crate::session::SessionRecord;

/// Every free-text field the redactor covers, as `<record>.<field>`. A unit
/// test pins this list against the fields [`Redactor`] actually masks; a new
/// free-text field joins both or the test fails.
pub const REDACTED_FIELDS: &[&str] = &[
    "entry.said",
    "entry.content",
    "session.title",
    "session.summary",
    "session.goals",
    "session.outcome",
    "session.next",
    "session.terms.term",
    "session.terms.said",
    "proposal.reason",
    "proposal.quotes.text",
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

    /// `text` with every configured value replaced by `[redacted:<NAME>]`.
    #[must_use]
    pub fn mask(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (name, value) in &self.secrets {
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), &format!("[redacted:{name}]"));
            }
        }
        out
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

    /// Mask a session record's free text.
    pub fn session(&self, record: &mut SessionRecord) {
        self.mask_all(session_fields(record));
    }

    /// Mask a proposal's free text.
    pub fn proposal(&self, proposal: &mut Proposal) {
        self.mask_all(proposal_fields(proposal));
    }
}

/// The free-text fields of an entry, named as in [`REDACTED_FIELDS`].
pub(super) fn entry_fields(entry: &mut Entry) -> Vec<(&'static str, &mut String)> {
    vec![
        ("entry.said", &mut entry.said),
        ("entry.content", &mut entry.content),
    ]
}

/// The free-text fields of a session record; each term is two fields.
pub(super) fn session_fields(record: &mut SessionRecord) -> Vec<(&'static str, &mut String)> {
    let mut fields = vec![
        ("session.title", &mut record.title),
        ("session.summary", &mut record.summary),
        ("session.goals", &mut record.goals),
        ("session.outcome", &mut record.outcome),
        ("session.next", &mut record.next),
    ];
    for term in &mut record.terms {
        fields.push(("session.terms.term", &mut term.term));
        fields.push(("session.terms.said", &mut term.said));
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
