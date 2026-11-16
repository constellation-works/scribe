//! The metadata every stored record carries in its frontmatter: where it was
//! recorded (`project`, `provider`, `host`), what it is about (`topics`) and
//! what it mentions (`related`), plus list selection shared by every record
//! kind.
//!
//! Pure values with validating constructors (STD-02 R14) and no I/O
//! (STD-02 R5): `scripts/check-dependency-direction.sh` rejects `std::fs`,
//! `std::net`, `std::process` and `crate::store` here. The serde shapes are
//! read from the persisted frontmatter, so a field is never renamed or
//! retyped without a store format bump (`store/format.rs::UPGRADES`).

use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::num::NonZeroUsize;
use std::str::FromStr;
use time::{Date, OffsetDateTime};

/// Why a value was rejected as record metadata.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordError {
    /// A project, provider or host was empty, too long or had whitespace.
    #[error(
        "invalid {what} '{value}': use 1-{max} characters with no spaces or control characters",
        max = Label::MAX_CHARS
    )]
    InvalidLabel {
        /// Which field: `project`, `provider` or `host`.
        what: &'static str,
        /// The rejected value.
        value: String,
    },
    /// A topic was not lowercase kebab-case.
    #[error(
        "invalid topic '{0}': use 1-{max} lowercase letters or digits in words joined by '-', such as ci or crew-routing",
        max = Topic::MAX_CHARS
    )]
    InvalidTopic(String),
    /// More topics than [`Topics::MAX`], or one repeated.
    #[error("a record carries at most {max} distinct topics; got {0}", max = Topics::MAX)]
    InvalidTopics(String),
    /// A related reference did not have a known `kind:` prefix and a target.
    #[error(
        "invalid related reference '{0}': expected task:<id>, pr:<owner>/<repo>#<n>, repo:<slug> or session:<id>"
    )]
    InvalidRelated(String),
    /// An author name was not 1-40 lowercase letters, digits, `.`, `_` or `-`.
    #[error(
        "invalid author name '{0}': use 1-{max} lowercase letters, digits, '.', '_' or '-'",
        max = AuthorName::MAX_CHARS
    )]
    InvalidAuthor(String),
}

/// Through which channel a record was written (2_design.md §3). `kind`
/// means the channel, not a claimed identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthorKind {
    /// A typed `#scribe` turn: Daniel's own words.
    Daniel,
    /// A CLI write verb, named by its `--author`.
    Agent,
    /// scribe's own routine (proposals and activity changes only).
    Scribe,
}

impl AuthorKind {
    /// The canonical lowercase token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daniel => "daniel",
            Self::Agent => "agent",
            Self::Scribe => "scribe",
        }
    }
}

/// An author's name: lowercase `[a-z0-9._-]`, 1-40 characters.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AuthorName(String);

impl AuthorName {
    /// The longest name accepted, in characters.
    pub const MAX_CHARS: usize = 40;

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this name belongs to a non-agent channel (`daniel`, or
    /// scribe's routine: `scribe` and its `consistency` and `session-pass` names), so a CLI
    /// write verb recording `kind: agent` may never use it. Reads still
    /// accept it as a filter.
    #[must_use]
    pub fn is_reserved(&self) -> bool {
        matches!(
            self.0.as_str(),
            "daniel" | "scribe" | "consistency" | "session-pass"
        )
    }
}

impl TryFrom<String> for AuthorName {
    type Error = RecordError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = !value.is_empty()
            && value.len() <= Self::MAX_CHARS
            && value.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            });
        if valid {
            Ok(Self(value))
        } else {
            Err(RecordError::InvalidAuthor(value))
        }
    }
}

impl From<AuthorName> for String {
    fn from(name: AuthorName) -> Self {
        name.0
    }
}

impl FromStr for AuthorName {
    type Err = RecordError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl fmt::Display for AuthorName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who wrote a record or a change: `{kind, name}` (store format 4).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Author {
    /// The channel.
    pub kind: AuthorKind,
    /// `daniel`, the agent's `--author`, or the routine's name.
    pub name: AuthorName,
}

impl Author {
    /// Daniel, through a typed `#scribe` turn.
    #[must_use]
    pub fn daniel() -> Self {
        Self {
            kind: AuthorKind::Daniel,
            name: AuthorName(String::from("daniel")),
        }
    }

    /// An agent or a person at a terminal, through a CLI write verb's
    /// `--author <name>`.
    #[must_use]
    pub fn agent(name: AuthorName) -> Self {
        Self {
            kind: AuthorKind::Agent,
            name,
        }
    }

    /// scribe's session pass, proposing unmarked user statements.
    #[must_use]
    pub(crate) fn session_pass() -> Self {
        Self {
            kind: AuthorKind::Scribe,
            name: AuthorName(String::from("session-pass")),
        }
    }

    /// scribe's consistency routine.
    #[must_use]
    pub fn consistency() -> Self {
        Self {
            kind: AuthorKind::Scribe,
            name: AuthorName(String::from("consistency")),
        }
    }
}

impl fmt::Display for Author {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind.as_str(), self.name)
    }
}

/// A short single-token label: the project slug, the provider, the host.
///
/// One token because the piped output form is one tab-separated line per
/// record (STD-01 R9): a tab or line break would split a record.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Label(String);

impl Label {
    /// The longest label accepted, in characters.
    pub const MAX_CHARS: usize = 128;

    /// The label text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `text` made into a label: each whitespace or control character
    /// becomes `-` and it is cut to [`Label::MAX_CHARS`]; an empty result is
    /// `fallback`. For values read from the host (its name, a directory
    /// name) that must become a label rather than be refused.
    #[must_use]
    pub fn sanitized(text: &str, fallback: &str) -> Self {
        let clean = |t: &str| -> String {
            t.chars()
                .map(|c| {
                    if c.is_whitespace() || c.is_control() {
                        '-'
                    } else {
                        c
                    }
                })
                .take(Self::MAX_CHARS)
                .collect()
        };
        let label = clean(text);
        if label.is_empty() {
            let fallback = clean(fallback);
            Self(if fallback.is_empty() {
                String::from("-")
            } else {
                fallback
            })
        } else {
            Self(label)
        }
    }

    fn parse_as(what: &'static str, value: String) -> Result<Self, RecordError> {
        let chars = value.chars().count();
        let valid = (1..=Self::MAX_CHARS).contains(&chars)
            && !value.chars().any(|c| c.is_whitespace() || c.is_control());
        if valid {
            Ok(Self(value))
        } else {
            Err(RecordError::InvalidLabel { what, value })
        }
    }
}

impl TryFrom<String> for Label {
    type Error = RecordError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse_as("label", value)
    }
}

impl From<Label> for String {
    fn from(label: Label) -> Self {
        label.0
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The repository slug a record belongs to (`orbit`,
/// `constellation-works/scribe`), or `global`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Project(Label);

impl Project {
    /// A record not tied to a repository.
    pub const GLOBAL: &'static str = "global";

    /// `text` made into a project label, as [`Label::sanitized`]; empty is
    /// [`Project::GLOBAL`].
    #[must_use]
    pub fn sanitized(text: &str) -> Self {
        Self(Label::sanitized(text, Self::GLOBAL))
    }

    /// The project text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<String> for Project {
    type Error = RecordError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Label::parse_as("project", value).map(Self)
    }
}

impl From<Project> for String {
    fn from(project: Project) -> Self {
        project.0.into()
    }
}

impl FromStr for Project {
    type Err = RecordError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl fmt::Display for Project {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A filter label: lowercase kebab-case words such as `ci` or
/// `crew-routing`. Distinct from a tag, which names where an entry goes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Topic(String);

impl Topic {
    /// The longest topic accepted, in characters.
    pub const MAX_CHARS: usize = 40;

    /// The topic text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Lowercase letters and digits in words joined by single hyphens.
pub(crate) fn is_kebab(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

impl TryFrom<String> for Topic {
    type Error = RecordError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if is_kebab(&value, Self::MAX_CHARS) {
            Ok(Self(value))
        } else {
            Err(RecordError::InvalidTopic(value))
        }
    }
}

impl From<Topic> for String {
    fn from(topic: Topic) -> Self {
        topic.0
    }
}

impl FromStr for Topic {
    type Err = RecordError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A record's topics: at most [`Topics::MAX`], none repeated, in the order
/// they were given.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<Topic>", into = "Vec<Topic>")]
pub struct Topics(Vec<Topic>);

impl Topics {
    /// The most topics one record carries.
    pub const MAX: usize = 5;

    /// The topics, in order.
    #[must_use]
    pub fn as_slice(&self) -> &[Topic] {
        &self.0
    }

    /// Whether `topic` is one of them.
    #[must_use]
    pub fn contains(&self, topic: &Topic) -> bool {
        self.0.contains(topic)
    }
}

impl TryFrom<Vec<Topic>> for Topics {
    type Error = RecordError;

    fn try_from(topics: Vec<Topic>) -> Result<Self, Self::Error> {
        let repeated = topics
            .iter()
            .enumerate()
            .any(|(i, t)| topics[..i].contains(t));
        if topics.len() > Self::MAX || repeated {
            let listed: Vec<&str> = topics.iter().map(Topic::as_str).collect();
            return Err(RecordError::InvalidTopics(listed.join(", ")));
        }
        Ok(Self(topics))
    }
}

impl From<Topics> for Vec<Topic> {
    fn from(topics: Topics) -> Self {
        topics.0
    }
}

/// What a [`Related`] reference points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelatedKind {
    /// An Orbit task.
    Task,
    /// A pull request, `<owner>/<repo>#<n>`.
    Pr,
    /// A repository slug.
    Repo,
    /// Another session record.
    Session,
}

impl RelatedKind {
    /// The `kind:` prefix, without the colon.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Pr => "pr",
            Self::Repo => "repo",
            Self::Session => "session",
        }
    }

    fn from_prefix(prefix: &str) -> Option<Self> {
        match prefix {
            "task" => Some(Self::Task),
            "pr" => Some(Self::Pr),
            "repo" => Some(Self::Repo),
            "session" => Some(Self::Session),
            _ => None,
        }
    }
}

/// A reference a record mentions: `task:<id>`, `pr:<owner>/<repo>#<n>`,
/// `repo:<slug>` or `session:<id>`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Related {
    kind: RelatedKind,
    target: String,
}

impl Related {
    /// What it points at.
    #[must_use]
    pub fn kind(&self) -> RelatedKind {
        self.kind
    }

    /// The part after the `kind:` prefix.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }
}

impl TryFrom<String> for Related {
    type Error = RecordError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let parsed = value.split_once(':').and_then(|(prefix, target)| {
            let kind = RelatedKind::from_prefix(prefix)?;
            let ok = !target.is_empty()
                && target.chars().count() <= Label::MAX_CHARS
                && !target.chars().any(|c| c.is_whitespace() || c.is_control());
            ok.then(|| Self {
                kind,
                target: target.to_owned(),
            })
        });
        parsed.ok_or(RecordError::InvalidRelated(value))
    }
}

impl FromStr for Related {
    type Err = RecordError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl From<Related> for String {
    fn from(related: Related) -> Self {
        related.to_string()
    }
}

impl fmt::Display for Related {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind.as_str(), self.target)
    }
}

/// A list's or search's `--since`/`--until`: calendar days, both ends
/// included. A record matches when a day it covers falls in the range: an
/// instant on that day, or a span (a session, a segment) that overlaps it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DateRange {
    /// The first day kept.
    pub since: Option<Date>,
    /// The last day kept.
    pub until: Option<Date>,
}

impl DateRange {
    /// A day as `YYYY-MM-DD`.
    #[must_use]
    pub fn day(date: Date) -> String {
        nullable_date::format(date)
    }

    /// Whether neither end is set.
    #[must_use]
    pub fn is_unbounded(&self) -> bool {
        self.since.is_none() && self.until.is_none()
    }

    /// Whether `at`'s day (in its own offset) is in the range.
    #[must_use]
    pub fn contains(&self, at: OffsetDateTime) -> bool {
        self.overlaps(at, at)
    }

    /// Whether the span from `from` to `to` has a day in the range. A span
    /// given backwards is read forwards.
    #[must_use]
    pub fn overlaps(&self, from: OffsetDateTime, to: OffsetDateTime) -> bool {
        let (first, last) = if from <= to {
            (from.date(), to.date())
        } else {
            (to.date(), from.date())
        };
        self.since.is_none_or(|since| last >= since)
            && self.until.is_none_or(|until| first <= until)
    }
}

/// Parse a calendar day, `YYYY-MM-DD`.
///
/// # Errors
///
/// A message naming the text and the expected form.
pub fn parse_day(text: &str) -> std::result::Result<Date, String> {
    Date::parse(text, nullable_date::FORMAT)
        .map_err(|_| format!("{text:?} is not a date in the form YYYY-MM-DD"))
}

/// What a listing found: the records returned and how many matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing<T> {
    /// The matching records, in the listing's order, cut to its limit.
    pub items: Vec<T>,
    /// How many records matched before the limit was applied.
    pub total: usize,
}

impl<T> Listing<T> {
    /// Keep the records `keep` accepts, in their given order, then cut to
    /// `limit`. The filter applies before the limit, and `total` counts every
    /// match, so a cut list can say how much it left out (STD-01 R33, R34).
    pub fn select(items: Vec<T>, keep: impl Fn(&T) -> bool, limit: Option<NonZeroUsize>) -> Self {
        let mut items: Vec<T> = items.into_iter().filter(|item| keep(item)).collect();
        let total = items.len();
        if let Some(limit) = limit {
            items.truncate(limit.get());
        }
        Self { items, total }
    }

    /// Whether the limit left matching records out (STD-01 R34).
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.items.len() < self.total
    }
}

/// Deserialize an `Option` whose key must be present: `null` is `None`, a
/// missing key is an error. serde treats a missing `Option` field as `None`
/// unless the field names its own deserializer, which this is; a record
/// with a missing frontmatter key is refused, not defaulted.
pub(crate) fn nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// `review_after` and other calendar dates: `YYYY-MM-DD`, `null` when
/// absent, the key always present.
pub(crate) mod nullable_date {
    use serde::{Deserialize, Deserializer, Serializer};
    use time::Date;
    use time::format_description::BorrowedFormatItem;
    use time::macros::format_description;

    pub(super) const FORMAT: &[BorrowedFormatItem<'static>] =
        format_description!("[year]-[month]-[day]");

    /// Render a date as `YYYY-MM-DD`.
    pub(crate) fn format(date: Date) -> String {
        date.format(FORMAT).unwrap_or_else(|_| date.to_string())
    }

    pub(crate) fn serialize<S: Serializer>(
        date: &Option<Date>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match date {
            Some(date) => serializer.serialize_str(&format(*date)),
            None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Date>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| Date::parse(&text, FORMAT).map_err(serde::de::Error::custom))
            .transpose()
    }
}

pub(crate) fn patch<T: Clone + PartialEq>(items: &mut Vec<T>, add: &[T], remove: &[T]) {
    items.retain(|item| !remove.contains(item));
    for item in add {
        if !items.contains(item) {
            items.push(item.clone());
        }
    }
}
