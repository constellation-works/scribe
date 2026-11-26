//! Shared record payload shapes, list JSON envelopes and conversion helpers
//! used across multiple commands (STD-01 R6, R10, R11).

use scribe_core::{Author, Cite, Instruction, Skipped};
use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use time::OffsetDateTime;

/// RFC 3339 with an explicit offset (STD-01 R11).
pub(crate) type Timestamp = OffsetDateTime;

/// Who wrote a record or a change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AuthorPayload {
    /// `daniel`, `agent` or `scribe`.
    pub(crate) kind: &'static str,
    pub(crate) name: String,
}

impl From<&Author> for AuthorPayload {
    fn from(author: &Author) -> Self {
        Self {
            kind: author.kind.as_str(),
            name: author.name.to_string(),
        }
    }
}

/// The turn a record was typed in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CitePayload {
    pub(crate) session: String,
    pub(crate) message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    pub(crate) prompt_sha256: String,
    pub(crate) line: u32,
    pub(crate) cwd: String,
}

impl From<&Cite> for CitePayload {
    fn from(cite: &Cite) -> Self {
        Self {
            session: cite.session.to_string(),
            message: cite.message.clone(),
            at: cite.at,
            prompt_sha256: cite.prompt_sha256.clone(),
            line: cite.line,
            cwd: cite.cwd.clone(),
        }
    }
}

/// The user's checked instruction that authorized an agent write (format 13).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct InstructionPayload {
    pub(crate) session: String,
    pub(crate) message: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    pub(crate) quote: Option<String>,
}

impl From<&Instruction> for InstructionPayload {
    fn from(inst: &Instruction) -> Self {
        Self {
            session: inst.session.to_string(),
            message: inst.message.clone(),
            at: inst.at,
            quote: inst.quote.clone(),
        }
    }
}

/// One field's origin: `shipped`, `config` (with the file and line) or
/// `none`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct OriginPayload {
    pub(crate) source: &'static str,
    /// The config file, for `config`; `null` otherwise.
    pub(crate) file: Option<String>,
    /// The 1-based line of the key, for `config`; `null` otherwise.
    pub(crate) line: Option<usize>,
}

impl OriginPayload {
    /// A default scribe ships.
    pub(crate) const SHIPPED: Self = Self {
        source: "shipped",
        file: None,
        line: None,
    };

    pub(crate) fn of<T>(rule: Option<&scribe_core::Rule<T>>, file: &str) -> Self {
        match rule.map(|r| r.origin) {
            None => Self {
                source: "none",
                file: None,
                line: None,
            },
            Some(scribe_core::RuleOrigin::Shipped) => Self {
                source: "shipped",
                file: None,
                line: None,
            },
            Some(scribe_core::RuleOrigin::Config { line }) => Self {
                source: "config",
                file: Some(file.to_owned()),
                line,
            },
        }
    }
}

/// A record file a list left out because it did not parse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WarningPayload {
    /// The file.
    pub(crate) path: String,
    /// The stable error code, `store_corrupt`.
    pub(crate) code: &'static str,
    /// What was wrong, naming the file.
    pub(crate) error: String,
}

impl From<&Skipped> for WarningPayload {
    fn from(skipped: &Skipped) -> Self {
        let mut error = skipped.error.to_string();
        let mut source = std::error::Error::source(&skipped.error);
        while let Some(cause) = source {
            error.push_str(": ");
            error.push_str(&cause.to_string());
            source = cause.source();
        }
        Self {
            path: skipped.path.display().to_string(),
            code: skipped.error.code(),
            error,
        }
    }
}

/// The `--json` form of a list: always `{"<records>": [...], "total",
/// "truncated", "warnings"}`, whatever the flags, so its shape never depends
/// on them (STD-01 R34, R10). Only a search that hints at stored messages
/// adds `message_matches` after them, so every other list is unchanged.
pub(crate) struct Envelope<'a, T> {
    pub(crate) key: &'static str,
    pub(crate) items: &'a [T],
    pub(crate) total: usize,
    pub(crate) warnings: &'a [WarningPayload],
    pub(crate) message_matches: Option<usize>,
}

impl<T: Serialize> Serialize for Envelope<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = if self.message_matches.is_some() { 5 } else { 4 };
        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry(self.key, self.items)?;
        map.serialize_entry("total", &self.total)?;
        map.serialize_entry("truncated", &(self.items.len() < self.total))?;
        map.serialize_entry("warnings", self.warnings)?;
        if let Some(count) = self.message_matches {
            map.serialize_entry("message_matches", &count)?;
        }
        map.end()
    }
}

pub(crate) fn envelope<'a, T>(
    key: &'static str,
    items: &'a [T],
    total: usize,
    warnings: &'a [WarningPayload],
) -> Envelope<'a, T> {
    Envelope {
        key,
        items,
        total,
        warnings,
        message_matches: None,
    }
}

pub(crate) fn strings<T: ToString>(items: &[T]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

/// `YYYY-MM-DD`.
pub(crate) fn date(date: time::Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// `user`, or `<kind>:<name>` for an agent's or scribe's entry.
pub(crate) fn author(a: &AuthorPayload) -> String {
    if a.kind == "user" {
        a.name.clone()
    } else {
        format!("{}:{}", a.kind, a.name)
    }
}

pub(crate) fn origin_text(o: &OriginPayload) -> String {
    match (&o.file, o.line) {
        (Some(file), Some(line)) => format!("{} {file}:{line}", o.source),
        (Some(file), None) => format!("{} {file}", o.source),
        (None, _) => o.source.to_owned(),
    }
}

/// What admitted a pass claim, on one line: the trigger as configured, the
/// turn's words it matched and the criteria in force.
pub(crate) fn admitted(a: &scribe_core::Admitted) -> String {
    format!(
        "trigger {:?} matched {:?}; criteria: {}",
        a.trigger,
        a.matched,
        a.criteria.as_deref().unwrap_or("-")
    )
}
