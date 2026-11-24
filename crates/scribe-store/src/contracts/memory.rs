//! The memory shapes the store reads and writes (docs/design/scribe/2_design.md
//! §15.5, §15.6): a `now` item and a milestone with their file stems and
//! front matter, an undated pending candidate, the profile as the block shows
//! it, and the codes a refused element is counted by. The memory app checks
//! and plans them.

use crate::record::nullable;
use crate::session::SessionId;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use time::{Date, OffsetDateTime};

/// The longest title, in characters.
pub const TITLE_CHARS: usize = 80;
/// The longest item text, in characters.
pub const TEXT_CHARS: usize = 400;
/// The longest slug of a file stem, in characters.
pub const SLUG_CHARS: usize = 48;

/// What a `now` item is: a deadline the user must meet, an event they
/// attend, or something they plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NowKind {
    /// A date by which something is due.
    Deadline,
    /// Something that happens on the date.
    Event,
    /// Something the user plans to do by or on the date.
    Plan,
}

impl NowKind {
    /// The kind's name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deadline => "deadline",
            Self::Event => "event",
            Self::Plan => "plan",
        }
    }
}

impl FromStr for NowKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "deadline" => Ok(Self::Deadline),
            "event" => Ok(Self::Event),
            "plan" => Ok(Self::Plan),
            other => Err(format!(
                "'{other}' is not a now kind; use deadline, event or plan"
            )),
        }
    }
}

/// A `now` or milestone file's stem and id: `<YYYY-MM-DD>-<slug>`, the slug
/// lowercase ASCII letters and digits in runs joined by `-`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Stem(String);

impl Stem {
    /// The stem for an item dated `date` titled `title`: the title's words
    /// as a slug, cut at [`SLUG_CHARS`] on a word, or `item` when it has
    /// none.
    #[must_use]
    pub fn new(date: Date, title: &str) -> Self {
        let mut slug = String::new();
        for word in title
            .to_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
        {
            let extra = usize::from(!slug.is_empty()) + word.len();
            if slug.len() + extra > SLUG_CHARS {
                break;
            }
            if !slug.is_empty() {
                slug.push('-');
            }
            slug.push_str(word);
        }
        if slug.is_empty() {
            slug.push_str("item");
        }
        Self(format!(
            "{}-{slug}",
            crate::record::nullable_date::format(date)
        ))
    }

    /// The stem text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The date the stem starts with.
    #[must_use]
    pub fn date(&self) -> Option<Date> {
        self.0
            .get(..10)
            .and_then(|d| crate::record::parse_day(d).ok())
    }
}

impl TryFrom<String> for Stem {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let slug = value.get(11..).unwrap_or_default();
        let valid = value.len() <= 11 + SLUG_CHARS
            && value
                .get(..10)
                .is_some_and(|d| crate::record::parse_day(d).is_ok())
            && value.as_bytes().get(10) == Some(&b'-')
            && !slug.is_empty()
            && !slug.starts_with('-')
            && !slug.ends_with('-')
            && !slug.contains("--")
            && slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if valid {
            Ok(Self(value))
        } else {
            Err(format!(
                "'{value}' is not a memory file stem (<YYYY-MM-DD>-<slug>)"
            ))
        }
    }
}

impl FromStr for Stem {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl From<Stem> for String {
    fn from(stem: Stem) -> Self {
        stem.0
    }
}

impl fmt::Display for Stem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// When an agent last raised a `now` item (`scribe memory ack`): it limits
/// unprompted mentions, never what the block shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Raised {
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// In which session.
    pub session: SessionId,
}

/// One `now` item: `memory/now/<stem>.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NowItem {
    /// Its file stem and id.
    pub stem: Stem,
    /// The event date or deadline.
    pub date: Date,
    /// What it is.
    pub kind: NowKind,
    /// The sessions it came from, in the order they were added.
    pub sources: Vec<SessionId>,
    /// When it last changed: the turn that wrote it.
    pub updated: OffsetDateTime,
    /// When an agent last raised it.
    pub raised: Option<Raised>,
    /// Its one-line title.
    pub title: String,
    /// Its prose.
    pub text: String,
}

/// One milestone: `memory/milestones/<stem>.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Milestone {
    /// Its file stem and id.
    pub stem: Stem,
    /// When it happened.
    pub date: Date,
    /// The sessions it came from.
    pub sources: Vec<SessionId>,
    /// When it last changed.
    pub updated: OffsetDateTime,
    /// Its one-line title.
    pub title: String,
    /// Its prose.
    pub text: String,
}

/// The frontmatter of a `now` file, as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NowFront {
    /// The event date or deadline.
    #[serde(with = "crate::record::nullable_date::required")]
    pub date: Date,
    /// What it is.
    pub kind: NowKind,
    /// The sessions it came from.
    pub sources: Vec<SessionId>,
    /// When it last changed.
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    /// When an agent last raised it.
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub raised: Option<Raised>,
}

/// The frontmatter of a milestone file, as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MilestoneFront {
    /// When it happened.
    #[serde(with = "crate::record::nullable_date::required")]
    pub date: Date,
    /// The sessions it came from.
    pub sources: Vec<SessionId>,
    /// When it last changed.
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

/// Why the pass refused a memory element; counted in the pass report by
/// code and audited by code only, never with its text (§13.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Refusal {
    /// `memory_invalid`: an unknown kind, or a field empty or past its
    /// bound.
    Invalid,
    /// `memory_unverified`: the quote is not the user's own verbatim span
    /// of the turn named, in this window and in the original transcript.
    Unverified,
    /// `memory_sensitive`: a credential or anything the redactor changes,
    /// or a listed finance word.
    Sensitive,
    /// `memory_inferred`: a `now` text naming a mood its quote does not.
    Inferred,
    /// `memory_undated`: a milestone, an update or a completion with no
    /// real date within a year before and two years after the turn.
    Undated,
    /// `memory_not_stated`: a milestone whose turn does not call it one.
    NotStated,
    /// `memory_unknown`: an update naming no listed `now` item, or a
    /// completion naming no pending candidate of this session.
    Unknown,
    /// `memory_pending_full`: the session already holds `MAX_PENDING`
    /// undated candidates.
    PendingFull,
}

impl Refusal {
    /// The stable code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "memory_invalid",
            Self::Unverified => "memory_unverified",
            Self::Sensitive => "memory_sensitive",
            Self::Inferred => "memory_inferred",
            Self::Undated => "memory_undated",
            Self::NotStated => "memory_not_stated",
            Self::Unknown => "memory_unknown",
            Self::PendingFull => "memory_pending_full",
        }
    }
}

/// An undated `now` candidate kept in `now_pending` (§15.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// Its row id; the model names it `pending-<id>` to date it.
    pub id: i64,
    /// The session it came from.
    pub session: SessionId,
    /// The turn it came from.
    pub message: String,
    /// What it is about: the element's title.
    pub subject: String,
    /// The element's prose.
    pub text: String,
}

/// The profile as the block shows it: its text and the date of the
/// revision it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileView {
    /// `profile.md`, or the committed revision while a journal is open.
    pub text: String,
    /// The day of its last revision; `None` for a file no revision wrote.
    pub as_of: Option<Date>,
}
