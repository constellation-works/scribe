//! Entries: Daniel's marked words and explicitly authored agent notes.
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

use crate::record::{
    Author, AuthorKind, AuthorName, DateRange, Label, Listing, Project, Related, Topic, Topics,
    nullable, nullable_date,
};
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
        "invalid tag '{0}': use 1-{max} lowercase letters or digits in words joined by '-', such as preferences",
        max = TagName::MAX_CHARS
    )]
    InvalidTag(String),
    /// The id was not `<tag>-<n>` with a positive `n`.
    #[error("invalid entry id '{0}': expected <tag>-<n>, such as preferences-12")]
    InvalidId(String),
    /// The status was not one of [`EntryStatus::NAMES`].
    #[error("invalid entry status '{0}': expected one of active, superseded, retired, forgotten")]
    InvalidStatus(String),
}

/// A tag's name: where an entry goes (`preferences`, `finding`).
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
/// let id: EntryId = "preferences-12".parse()?;
/// assert_eq!(id.tag().as_str(), "preferences");
/// assert_eq!(id.number(), 12);
/// assert!("preferences-0".parse::<EntryId>().is_err(), "numbers start at 1");
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
    /// Blanked for good: `said` and `content` are empty, and `evidence`,
    /// `as_of`, `quote`, `window` and `retirement_reason` are cleared; id and
    /// cite stay.
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
    /// Written directly through an agent CLI verb.
    Agent,
    /// A verbatim substring of the `#scribe` line itself.
    Instruction,
    /// A verbatim span of an earlier message selected by a referent
    /// instruction or an approved session-pass candidate.
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

/// The provenance of an entry: the original turn holding its words.
/// An approved candidate keeps this source turn; its proposal separately
/// records the typed approval in `settled_by`.
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
    /// The source line's number within the prompt, from 1.
    pub line: u32,
    /// The session's working directory.
    pub cwd: String,
}

/// Daniel's checked typed instruction that authorized an agent to settle a
/// claim or retire or forget an entry (docs/design/scribe/2_design.md §12.3, §12.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    /// The interactive session he typed it in.
    pub session: SessionId,
    /// The user message's uuid.
    pub message: String,
    /// When he typed it.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The quoted span of his turn, redacted; `null` once the entry is forgotten.
    #[serde(deserialize_with = "nullable")]
    pub quote: Option<String>,
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
    /// Who wrote it: `{kind: daniel}` for a typed `#scribe` line.
    pub author: Author,
    /// Who last updated or retired it through a CLI verb, or `null`.
    #[serde(deserialize_with = "nullable")]
    pub updated_by: Option<Author>,
    /// When it was recorded.
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    /// When it last changed.
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    /// When it stopped being `active`: the retirement, supersession or
    /// forgetting that took it out (format 12). `null` while active. Later
    /// metadata writes move `updated`, never this, so Stop recovery can
    /// tell which turns the entry was still active for.
    #[serde(with = "time::serde::rfc3339::option")]
    pub inactive_since: Option<OffsetDateTime>,
    /// The full `#scribe` line exactly as Daniel typed it, or a candidate
    /// approval's canonical command; empty once forgotten.
    pub said: String,
    /// Where `content` came from.
    pub content_from: ContentFrom,
    /// The typed turn; null for an agent note.
    #[serde(deserialize_with = "nullable")]
    pub cite: Option<Cite>,
    /// Redacted reason for direct retirement, or null.
    #[serde(deserialize_with = "nullable")]
    pub retirement_reason: Option<String>,
    /// The entry that replaced it, when `superseded`.
    #[serde(deserialize_with = "nullable")]
    pub superseded_by: Option<EntryId>,
    /// When it is due for review, from the tag's `stale_after_days`.
    #[serde(with = "nullable_date")]
    pub review_after: Option<Date>,
    /// An agent entry's quote of Daniel's correction that prompted it,
    /// verbatim from one of Daniel's typed turns in the session the entry names
    /// (format 8); `null` otherwise.
    #[serde(deserialize_with = "nullable")]
    pub quote: Option<String>,
    /// The chat leading up to a typed `#scribe` line under an action tag
    /// (format 8); `null` for every other entry.
    #[serde(deserialize_with = "nullable")]
    pub window: Option<Window>,
    /// A finding's evidence pointer: a path, task, run, experiment or
    /// commit the result can be checked against (format 10); `null` for
    /// every entry not under a `require_evidence` tag.
    #[serde(deserialize_with = "nullable")]
    pub evidence: Option<String>,
    /// The date a finding's result was true (format 10); `null` beside a
    /// `null` evidence pointer.
    #[serde(with = "nullable_date")]
    pub as_of: Option<Date>,
    /// The agent that approved this candidate on Daniel's checked instruction
    /// (format 13); `null` when Daniel typed `#scribe approve` or for direct entries.
    #[serde(deserialize_with = "nullable")]
    pub approved_by: Option<Author>,
    /// The checked instruction that approved, retired or forgot this entry
    /// (format 13); `null` otherwise.
    #[serde(deserialize_with = "nullable")]
    pub instruction: Option<Instruction>,
    /// Whose words `content` is when they are not the author's own
    /// (format 14): `{kind: scribe, name: session-pass}` for an approved
    /// pass claim whose content is scribe's statement of Daniel's turn,
    /// with his verbatim words in `quote`; `null` for every other entry.
    #[serde(deserialize_with = "nullable")]
    pub stated_by: Option<Author>,
    /// Where an idea the session pass captured stands (format 15): `open`,
    /// `parked` with Daniel's own words setting it aside, or `task` with
    /// the task it became. Set only on an entry the pass wrote itself
    /// (`author` `{kind: scribe, name: session-pass}`); `null` for every
    /// other entry.
    #[serde(deserialize_with = "nullable")]
    pub outcome: Option<Outcome>,
    /// What the entry records, verbatim: the file's body, not a frontmatter
    /// key. Empty once forgotten.
    #[serde(skip)]
    pub content: String,
}

/// Where a pass-captured idea stands (docs/design/scribe/2_design.md
/// §11.6): every key is required, `null` where it does not apply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    /// `open`, `parked` or `task`.
    pub state: OutcomeState,
    /// For `parked`, Daniel's own words setting the idea aside, verbatim
    /// from one of his turns in the cited session; `null` otherwise and
    /// once the entry is forgotten.
    #[serde(deserialize_with = "nullable")]
    pub reason: Option<String>,
    /// For `task`, the id of the task the idea became; `null` otherwise.
    #[serde(deserialize_with = "nullable")]
    pub task: Option<String>,
}

impl Outcome {
    /// An idea nothing has settled yet.
    #[must_use]
    pub fn open() -> Self {
        Self {
            state: OutcomeState::Open,
            reason: None,
            task: None,
        }
    }

    /// Whether the fields agree with the state: only `parked` carries a
    /// reason (blank once forgotten, so `None` is accepted when
    /// `forgotten`), and only `task` a non-empty task id.
    #[must_use]
    pub fn is_consistent(&self, forgotten: bool) -> bool {
        let reason_ok = |r: &Option<String>| match r {
            Some(text) => !text.trim().is_empty() && !forgotten,
            None => forgotten,
        };
        match self.state {
            OutcomeState::Open => self.reason.is_none() && self.task.is_none(),
            OutcomeState::Parked => self.task.is_none() && reason_ok(&self.reason),
            OutcomeState::Task => {
                self.reason.is_none() && self.task.as_ref().is_some_and(|t| !t.trim().is_empty())
            }
        }
    }
}

/// A pass-captured idea's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutcomeState {
    /// Nothing has settled it.
    Open,
    /// Daniel set it aside, in his words.
    Parked,
    /// It became a task.
    Task,
}

impl OutcomeState {
    /// The canonical lowercase token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Parked => "parked",
            Self::Task => "task",
        }
    }
}

impl fmt::Display for OutcomeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The bounded excerpt of the chat before a typed `#scribe` line under an
/// action tag: the turns since the previous `#scribe` line of the session,
/// oldest first, then the prompt itself (docs/design/scribe/2_design.md
/// §4.6). Its size is capped by [`crate::capture::WINDOW_TURNS`] and
/// [`crate::capture::WINDOW_CHARS`]; turns past the cap are dropped from the
/// oldest end and `truncated` says so.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    /// The turns kept, oldest first.
    pub turns: Vec<WindowTurn>,
    /// Whether older turns were dropped to fit the cap.
    pub truncated: bool,
    /// How many older turns were dropped.
    pub omitted: u32,
}

/// One turn of a [`Window`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowTurn {
    /// Whose words they are.
    pub role: Speaker,
    /// The transcript message's uuid; `null` for the prompt the line was
    /// typed in, which is not in the transcript yet when the hook runs.
    #[serde(deserialize_with = "nullable")]
    pub message: Option<String>,
    /// When it was sent.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// Its text, cut to [`crate::capture::WINDOW_TURN_CHARS`].
    pub text: String,
    /// Whether `text` was cut.
    pub cut: bool,
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
    /// Only entries carrying every one of these topics.
    pub topics: Vec<Topic>,
    /// Only entries mentioning at least one of these references (none set:
    /// any entry).
    pub related: Vec<Related>,
    /// Only entries created on a day in this range.
    pub dates: DateRange,
    /// Only entries created strictly after this instant: an intake's cursor.
    pub after: Option<OffsetDateTime>,
    /// Only entries written under this author name.
    pub author: Option<AuthorName>,
    /// Only entries written through this channel.
    pub author_kind: Option<AuthorKind>,
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
            && self.topics.iter().all(|t| entry.topics.contains(t))
            && (self.related.is_empty() || self.related.iter().any(|r| entry.related.contains(r)))
            && self.dates.contains(entry.created)
            && self.after.is_none_or(|after| entry.created > after)
            && self.author.as_ref().is_none_or(|a| &entry.author.name == a)
            && self.author_kind.is_none_or(|k| entry.author.kind == k)
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
/// replace nothing, are `updated` when `created`, and are Daniel's: a typed
/// `#scribe` line is the only way to write one.
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
    /// The chat before the line, under an action tag only.
    pub window: Option<Window>,
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
            author: Author::daniel(),
            updated_by: None,
            created: self.created,
            updated: self.created,
            inactive_since: None,
            said: self.said,
            content_from: self.content_from,
            cite: Some(self.cite),
            retirement_reason: None,
            superseded_by: None,
            review_after: self.review_after,
            quote: None,
            window: self.window,
            evidence: None,
            as_of: None,
            approved_by: None,
            instruction: None,
            stated_by: None,
            outcome: None,
            content: self.content,
        }
    }
}

impl Entry {
    /// Whether this entry is a finding: it carries an evidence pointer,
    /// which only a `require_evidence` tag's agent entries may.
    #[must_use]
    pub fn is_finding(&self) -> bool {
        self.evidence.is_some()
    }

    /// The session this entry names, for the per-session finding cap: a
    /// `session:` reference it carries, else its cite's session.
    #[must_use]
    pub fn session_key(&self) -> Option<String> {
        self.related
            .iter()
            .find(|r| r.kind() == crate::record::RelatedKind::Session)
            .map(|r| r.target().to_owned())
            .or_else(|| self.cite.as_ref().map(|c| c.session.to_string()))
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

/// Metadata and content for a directly written note.
#[derive(Clone, Debug)]
pub struct AgentEntry {
    /// Tag name or alias.
    pub tag: TagName,
    /// Content to redact and store.
    pub content: String,
    /// Project, or global.
    pub project: Project,
    /// Topics.
    pub topics: Topics,
    /// Related references.
    pub related: Vec<Related>,
    /// Host supplied by composition.
    pub host: Label,
    /// Daniel's correction that prompted it, verbatim; required under a
    /// tag configured with `require_quote`, refused under a memory tag.
    pub quote: Option<String>,
    /// A finding's evidence pointer; required under a `require_evidence`
    /// tag, refused under any other tag.
    pub evidence: Option<String>,
    /// The date the finding was true; `None` takes the write's date.
    pub as_of: Option<Date>,
}

/// A metadata/content patch. Removals apply before additions.
#[derive(Clone, Debug, Default)]
pub struct EntryPatch {
    /// Replacement content, only for agent notes.
    pub content: Option<String>,
    /// Topics to add.
    pub add_topics: Vec<Topic>,
    /// Topics to remove.
    pub remove_topics: Vec<Topic>,
    /// References to add.
    pub add_related: Vec<Related>,
    /// References to remove.
    pub remove_related: Vec<Related>,
}
