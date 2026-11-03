//! Session records, the on/off control state, and the rule that marks a
//! session pass due.
//!
//! A session record is one file, `sessions/<yyyy>/<mm>/<id>.md`: YAML
//! frontmatter and four Markdown sections, `## Summary`, `## Goals`,
//! `## Outcome` and `## Next` (docs/design/scribe/2_design.md §3). The text
//! is model-written: `ask` labels it a session summary and never quotes it
//! as Daniel's words. Pure values (STD-02 R14) and no I/O (STD-02 R5).
//!
//! As for entries, every frontmatter key is required and an unknown one is
//! refused; never rename or retype a field without a store format bump.

use crate::record::{Label, Listing, Project, Related, Topic, Topics, nullable};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::num::{NonZeroU32, NonZeroUsize};
use std::str::FromStr;
use time::{Duration, OffsetDateTime};

/// Why a value was rejected as a session field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// The id had characters outside letters, digits, `.`, `_` and `-`.
    #[error(
        "invalid session id '{0}': use 1-{max} letters, digits, '.', '_' or '-', not starting with '.'",
        max = SessionId::MAX_CHARS
    )]
    InvalidId(String),
}

/// A Claude Code session id (a uuid in practice). Also the record's file
/// name, so it is restricted to characters that are safe in one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

impl SessionId {
    /// The longest id accepted, in characters.
    pub const MAX_CHARS: usize = 128;

    /// The id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SessionId {
    type Error = SessionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = !value.is_empty()
            && value.len() <= Self::MAX_CHARS
            && !value.starts_with('.')
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if valid {
            Ok(Self(value))
        } else {
            Err(SessionError::InvalidId(value))
        }
    }
}

impl From<SessionId> for String {
    fn from(id: SessionId) -> Self {
        id.0
    }
}

impl FromStr for SessionId {
    type Err = SessionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether more passes will update a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    /// The session may still be updated.
    Open,
    /// The session-end pass ran; the record is complete.
    Final,
}

impl SessionStatus {
    /// The canonical lowercase token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Final => "final",
        }
    }
}

/// The four body sections, in file order, with their character budgets.
/// A pass that writes more than a budget is refused (S4); a stored record is
/// read whatever its length.
pub const SECTIONS: [(&str, usize); 4] = [
    ("Summary", 1500),
    ("Goals", 600),
    ("Outcome", 2500),
    ("Next", 1000),
];

/// A phrase Daniel coined or defined in a session, kept in his exact words
/// so `ask` can find where he said it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Term {
    /// The phrase, as Daniel writes it (`graduate to level 4`).
    pub term: String,
    /// A verbatim span of Daniel's own turn that defines or first uses it.
    pub said: String,
    /// The uuid of that turn's message.
    pub message: String,
}

/// A stored session record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecord {
    /// The session id; also the file name.
    pub id: SessionId,
    /// Whether more passes will update it.
    pub status: SessionStatus,
    /// The repository the session ran in.
    pub project: Project,
    /// Filter labels the session model chose, reusing existing ones first.
    pub topics: Topics,
    /// References the session mentions.
    pub related: Vec<Related>,
    /// Phrases Daniel coined or defined in the session, in his words.
    pub terms: Vec<Term>,
    /// The tool, such as `claude-code`.
    pub provider: Label,
    /// The host it ran on.
    pub host: Label,
    /// When the record was first written.
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    /// When a pass last rewrote it.
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    /// A one-line title, model-written.
    pub title: String,
    /// The session's working directory.
    pub cwd: String,
    /// When the session started; it also names the `<yyyy>/<mm>` directory.
    #[serde(with = "time::serde::rfc3339")]
    pub started: OffsetDateTime,
    /// When Daniel switched cataloging on.
    #[serde(with = "time::serde::rfc3339")]
    pub on_since: OffsetDateTime,
    /// The uuid of the last transcript message folded in.
    #[serde(deserialize_with = "nullable")]
    pub scribed_through: Option<String>,
    /// How many passes have written it.
    pub passes: u32,
    /// How many items `next` lists.
    pub next_open: u32,
    /// `## Summary`.
    #[serde(skip)]
    pub summary: String,
    /// `## Goals`.
    #[serde(skip)]
    pub goals: String,
    /// `## Outcome`.
    #[serde(skip)]
    pub outcome: String,
    /// `## Next`.
    #[serde(skip)]
    pub next: String,
}

/// Which session records a listing returns. Every field set must match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFilter {
    /// Only sessions in this project.
    pub project: Option<Project>,
    /// Only sessions carrying this topic.
    pub topic: Option<Topic>,
    /// At most this many of the matching sessions.
    pub limit: Option<NonZeroUsize>,
}

impl SessionFilter {
    /// Whether `record` passes every criterion (the limit aside).
    #[must_use]
    pub fn matches(&self, record: &SessionRecord) -> bool {
        self.project.as_ref().is_none_or(|p| &record.project == p)
            && self
                .topic
                .as_ref()
                .is_none_or(|t| record.topics.contains(t))
    }

    /// Apply the filter to `records`, oldest first (by `started`, then id),
    /// then the limit.
    #[must_use]
    pub fn select(&self, mut records: Vec<SessionRecord>) -> Listing<SessionRecord> {
        records.sort_by(|a, b| a.started.cmp(&b.started).then_with(|| a.id.cmp(&b.id)));
        Listing::select(records, |r| self.matches(r), self.limit)
    }
}

/// One session's cataloging switch, in `control.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionControl {
    /// Whether cataloging is on.
    pub on: bool,
    /// When it was last switched on.
    #[serde(with = "time::serde::rfc3339::option")]
    pub since: Option<OffsetDateTime>,
    /// Whether a pass is due.
    pub due: bool,
    /// Whether the due pass is the session-end one.
    #[serde(rename = "final")]
    pub final_pass: bool,
}

/// Every session's switch, keyed by session id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlState {
    /// The sessions that were ever switched on.
    pub sessions: BTreeMap<SessionId, SessionControl>,
}

impl ControlState {
    /// The switch for `id`; a session never switched on has none.
    #[must_use]
    pub fn get(&self, id: &SessionId) -> Option<&SessionControl> {
        self.sessions.get(id)
    }
}

/// How often an opted-in session gets a pass: `[catalog]` in `config.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence {
    /// Minutes since the last pass.
    pub every_minutes: NonZeroU32,
    /// User turns since the last pass.
    pub every_turns: NonZeroU32,
}

impl Default for Cadence {
    fn default() -> Self {
        Self {
            every_minutes: NonZeroU32::new(20).unwrap_or(NonZeroU32::MIN),
            every_turns: NonZeroU32::new(10).unwrap_or(NonZeroU32::MIN),
        }
    }
}

impl Cadence {
    /// The due rule the `Stop` hook applies after each reply: a session that
    /// is on needs a pass once `every_minutes` have passed since
    /// `last_pass`, or `every_turns` user turns have arrived since it. A
    /// session that is off is never due; one already due stays due.
    #[must_use]
    pub fn pass_due(
        &self,
        control: &SessionControl,
        last_pass: OffsetDateTime,
        now: OffsetDateTime,
        turns_since: u32,
    ) -> bool {
        if !control.on {
            return false;
        }
        let waited = now - last_pass >= Duration::minutes(i64::from(self.every_minutes.get()));
        control.due || waited || turns_since >= self.every_turns.get()
    }
}
