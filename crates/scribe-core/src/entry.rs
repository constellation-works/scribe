//! Entries: the decisions and preferences Daniel marks with `#scribe`.
//!
//! An entry is one file, `entries/<tag>/<id>.md`: YAML frontmatter holding
//! the fields below and a body that is [`Entry::content`], verbatim
//! (docs/design/scribe/2_design.md §3). Pure values with validating
//! constructors (STD-02 R14) and no I/O (STD-02 R5).
//!
//! The serde shapes are the persisted frontmatter (STD-02 R16): every key is
//! required (`null` where a value may be absent) and an unknown key is
//! refused, so a hand-edited file with a typo is reported, not half-read.
//! Never rename or retype a field without a store format bump.

use crate::record::{Label, Listing, Project, Related, Topic, Topics, nullable, nullable_date};
use crate::session::SessionId;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::num::{NonZeroU64, NonZeroUsize};
use std::str::FromStr;
use time::{Date, OffsetDateTime};

/// Why a value was rejected as an entry field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EntryError {
    /// The tag had characters outside `a-z`, `0-9` and `-`, or a bad length.
    #[error(
        "invalid tag '{0}': use 1-{max} lowercase letters or digits in words joined by '-', such as decisions",
        max = TagName::MAX_CHARS
    )]
    InvalidTag(String),
    /// The id was not `<tag>-<n>` with a positive `n`.
    #[error("invalid entry id '{0}': expected <tag>-<n>, such as decisions-12")]
    InvalidId(String),
    /// The status was not one of [`EntryStatus::NAMES`].
    #[error("invalid entry status '{0}': expected one of active, superseded, retired, forgotten")]
    InvalidStatus(String),
}

/// A tag's name: where an entry goes (`decisions`, `preferences`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TagName(String);

impl TagName {
    /// The longest tag name accepted, in characters.
    pub const MAX_CHARS: usize = 32;

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TagName {
    type Error = EntryError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if crate::record::is_kebab(&value, Self::MAX_CHARS) {
            Ok(Self(value))
        } else {
            Err(EntryError::InvalidTag(value))
        }
    }
}

impl From<TagName> for String {
    fn from(tag: TagName) -> Self {
        tag.0
    }
}

impl FromStr for TagName {
    type Err = EntryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl fmt::Display for TagName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An entry's id, `<tag>-<n>`: short, self-describing in an echo, and never
/// reused within its tag.
///
/// ```
/// use scribe_core::EntryId;
///
/// let id: EntryId = "decisions-12".parse()?;
/// assert_eq!(id.tag().as_str(), "decisions");
/// assert_eq!(id.number(), 12);
/// assert!("decisions-0".parse::<EntryId>().is_err(), "numbers start at 1");
/// # Ok::<(), scribe_core::EntryError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EntryId {
    tag: TagName,
    number: NonZeroU64,
}

impl EntryId {
    /// The id `<tag>-<number>`.
    #[must_use]
    pub fn new(tag: TagName, number: NonZeroU64) -> Self {
        Self { tag, number }
    }

    /// The tag the entry belongs to.
    #[must_use]
    pub fn tag(&self) -> &TagName {
        &self.tag
    }

    /// The number within the tag.
    #[must_use]
    pub fn number(&self) -> u64 {
        self.number.get()
    }

    /// The id for the next entry in `tag`: one past the highest number among
    /// `existing` ids in that tag, or `<tag>-1`. Ids of other tags are
    /// ignored. The store calls this under its lock, so a number is never
    /// handed out twice; a retired or forgotten entry keeps its number.
    pub fn next_in<'a>(tag: &TagName, existing: impl IntoIterator<Item = &'a EntryId>) -> Self {
        let highest = existing
            .into_iter()
            .filter(|id| &id.tag == tag)
            .map(|id| id.number.get())
            .max()
            .unwrap_or(0);
        Self {
            tag: tag.clone(),
            number: NonZeroU64::new(highest.saturating_add(1)).unwrap_or(NonZeroU64::MAX),
        }
    }
}

impl TryFrom<String> for EntryId {
    type Error = EntryError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let invalid = || EntryError::InvalidId(value.clone());
        let (tag, number) = value.rsplit_once('-').ok_or_else(invalid)?;
        // `+12` and `012` parse as numbers but would not round-trip.
        if !number.bytes().all(|b| b.is_ascii_digit()) || number.starts_with('0') {
            return Err(invalid());
        }
        let number: NonZeroU64 = number.parse().map_err(|_| invalid())?;
        let tag = TagName::try_from(tag.to_owned()).map_err(|_| invalid())?;
        Ok(Self { tag, number })
    }
}

impl From<EntryId> for String {
    fn from(id: EntryId) -> Self {
        id.to_string()
    }
}

impl FromStr for EntryId {
    type Err = EntryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl fmt::Display for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.tag, self.number)
    }
}

/// Where an entry stands. Only `active` entries are `ask` evidence by
/// default; the others are hidden, never deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryStatus {
    /// Current.
    Active,
    /// A newer entry replaced it (`superseded_by` names it).
    Superseded,
    /// Hidden by Daniel or by an approved stale proposal.
    Retired,
    /// Blanked for good: `said` and `content` are empty; id and cite stay.
    Forgotten,
}

impl EntryStatus {
    /// Every status's canonical token. Surfaces list allowed values from here
    /// rather than repeating them (STD-01 R25).
    pub const NAMES: [&'static str; 4] = ["active", "superseded", "retired", "forgotten"];

    /// The canonical lowercase token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Retired => "retired",
            Self::Forgotten => "forgotten",
        }
    }

    /// Whether `ask` leaves it out unless asked for hidden records.
    #[must_use]
    pub fn is_hidden(self) -> bool {
        !matches!(self, Self::Active)
    }
}

impl FromStr for EntryStatus {
    type Err = EntryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "superseded" => Ok(Self::Superseded),
            "retired" => Ok(Self::Retired),
            "forgotten" => Ok(Self::Forgotten),
            other => Err(EntryError::InvalidStatus(other.to_owned())),
        }
    }
}

impl fmt::Display for EntryStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whose words a message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    /// Daniel.
    User,
    /// The assistant; `ask` never presents these words as Daniel's.
    Assistant,
}

impl Speaker {
    /// The canonical lowercase token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// Where an entry's content came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ContentFrom {
    /// A verbatim substring of the `#scribe` line itself.
    Instruction,
    /// A verbatim span of an earlier message the instruction pointed at
    /// ("this", "it").
    Message {
        /// Whose words they are.
        role: Speaker,
        /// The transcript message's uuid.
        message: String,
        /// When that message was sent.
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
    },
}

/// The provenance of an entry: the turn Daniel typed it in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cite {
    /// The Claude Code session.
    pub session: SessionId,
    /// The user message's uuid; `null` until the next `Stop` hook finds it.
    #[serde(deserialize_with = "nullable")]
    pub message: Option<String>,
    /// When the prompt was submitted.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// SHA-256 of the prompt text, lowercase hex.
    pub prompt_sha256: String,
    /// The `#scribe` line's number within the prompt, from 1.
    pub line: u32,
    /// The session's working directory.
    pub cwd: String,
}

/// A stored entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// `<tag>-<n>`; the file is `entries/<tag>/<id>.md`.
    pub id: EntryId,
    /// The tag; always the id's tag.
    pub tag: TagName,
    /// Where it stands.
    pub status: EntryStatus,
    /// The repository it was recorded in, or `global`.
    pub project: Project,
    /// Filter labels: the extra hashtags on the `#scribe` line.
    pub topics: Topics,
    /// References it mentions.
    pub related: Vec<Related>,
    /// The tool it was recorded through, such as `claude-code`.
    pub provider: Label,
    /// The host it was recorded on.
    pub host: Label,
    /// When it was recorded.
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    /// When it last changed.
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    /// The full `#scribe` line exactly as Daniel typed it; empty once
    /// forgotten.
    pub said: String,
    /// Where `content` came from.
    pub content_from: ContentFrom,
    /// The turn it was typed in.
    pub cite: Cite,
    /// The entry that replaced it, when `superseded`.
    #[serde(deserialize_with = "nullable")]
    pub superseded_by: Option<EntryId>,
    /// When it is due for review, from the tag's `stale_after_days`.
    #[serde(with = "nullable_date")]
    pub review_after: Option<Date>,
    /// What the entry records, verbatim: the file's body, not a frontmatter
    /// key. Empty once forgotten.
    #[serde(skip)]
    pub content: String,
}

/// Which entries a listing returns. Every field set must match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryFilter {
    /// Only entries in this tag.
    pub tag: Option<TagName>,
    /// Only entries with this status.
    pub status: Option<EntryStatus>,
    /// Only entries recorded in this project.
    pub project: Option<Project>,
    /// Only entries carrying this topic.
    pub topic: Option<Topic>,
    /// At most this many of the matching entries.
    pub limit: Option<NonZeroUsize>,
}

impl EntryFilter {
    /// Whether `entry` passes every criterion (the limit aside).
    #[must_use]
    pub fn matches(&self, entry: &Entry) -> bool {
        self.tag.as_ref().is_none_or(|t| &entry.tag == t)
            && self.status.is_none_or(|s| entry.status == s)
            && self.project.as_ref().is_none_or(|p| &entry.project == p)
            && self.topic.as_ref().is_none_or(|t| entry.topics.contains(t))
    }

    /// Apply the filter to `entries`, oldest first (by `created`, then id),
    /// then the limit.
    #[must_use]
    pub fn select(&self, mut entries: Vec<Entry>) -> Listing<Entry> {
        entries.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
        Listing::select(entries, |e| self.matches(e), self.limit)
    }
}

/// An entry before the store gives it an id: what `#scribe` capture builds
/// from one instruction (`capture::draft`). New entries are always `active`,
/// replace nothing and are `updated` when `created`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewEntry {
    /// The tag; the id is numbered within it.
    pub tag: TagName,
    /// The repository it was recorded in.
    pub project: Project,
    /// The extra hashtags on the line.
    pub topics: Topics,
    /// References it mentions.
    pub related: Vec<Related>,
    /// The tool it was recorded through.
    pub provider: Label,
    /// The host it was recorded on.
    pub host: Label,
    /// When it was recorded.
    pub created: OffsetDateTime,
    /// The `#scribe` line as typed.
    pub said: String,
    /// What it records, verbatim.
    pub content: String,
    /// Where `content` came from.
    pub content_from: ContentFrom,
    /// The turn it was typed in; `message` is still `None`.
    pub cite: Cite,
    /// When it is due for review.
    pub review_after: Option<Date>,
}

impl NewEntry {
    /// The stored entry under `id`.
    #[must_use]
    pub fn into_entry(self, id: EntryId) -> Entry {
        Entry {
            id,
            tag: self.tag,
            status: EntryStatus::Active,
            project: self.project,
            topics: self.topics,
            related: self.related,
            provider: self.provider,
            host: self.host,
            created: self.created,
            updated: self.created,
            said: self.said,
            content_from: self.content_from,
            cite: self.cite,
            superseded_by: None,
            review_after: self.review_after,
            content: self.content,
        }
    }
}

impl Cite {
    /// Whether `other` cites the same `#scribe` line: the same session,
    /// prompt and line. This is capture's idempotency key
    /// (docs/design/scribe/2_design.md §4.2), so a retried hook finds the
    /// entry it already wrote instead of writing a second.
    #[must_use]
    pub fn same_line(&self, other: &Cite) -> bool {
        self.session == other.session
            && self.prompt_sha256 == other.prompt_sha256
            && self.line == other.line
    }
}
