//! Typed decision and open-item fields (2_design.md §18).

use super::EntryId;
use crate::record::AuthorName;
use crate::session::SessionId;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use time::OffsetDateTime;

pub(crate) const DECISION_TAG: &str = "decision";
pub(crate) const OPEN_ITEM_TAG: &str = "open-item";

pub(crate) fn is_decision_or_item(tag: &str) -> bool {
    matches!(tag, DECISION_TAG | OPEN_ITEM_TAG)
}

/// A choice that outlives its task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    /// `user`, or the session name of the agent who decided.
    pub decided_by: AuthorName,
    /// The agent's reason, for an agent decision.
    pub rationale: Option<String>,
    /// The user's reason, verbatim, if supplied for a user decision.
    pub reason_quote: Option<String>,
    /// The full options that were presented.
    pub options: Vec<String>,
    /// The 1-based recommendation, if there was one.
    pub recommended: Option<usize>,
    /// The active decision this entry replaced at creation.
    pub supersedes: Option<EntryId>,
    /// The open item this decision answers.
    pub settles: Option<EntryId>,
}

impl Decision {
    /// Whether the choice was the user's.
    #[must_use]
    pub fn by_user(&self) -> bool {
        self.decided_by.as_str() == "user"
    }

    pub(crate) fn chars(&self, content: &str, quote: Option<&str>) -> usize {
        std::iter::once(content)
            .chain(self.options.iter().map(String::as_str))
            .chain(self.rationale.as_deref())
            .chain(self.reason_quote.as_deref())
            .chain(quote)
            .map(|text| text.chars().count())
            .sum()
    }
}

/// Where the user can answer an open item.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum AnswerIn {
    /// Any session can take the answer.
    #[default]
    Any,
    /// The answer must also be typed in this session for it to act.
    Session(SessionId),
    /// The user takes the action directly.
    Hand,
}

/// An invalid open-item answer location.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("expected any, hand, or session:<id> for --answer-in")]
pub struct InvalidAnswerIn;

impl FromStr for AnswerIn {
    type Err = InvalidAnswerIn;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "any" => Ok(Self::Any),
            "hand" => Ok(Self::Hand),
            _ => value
                .strip_prefix("session:")
                .and_then(|id| id.parse().ok())
                .map(Self::Session)
                .ok_or(InvalidAnswerIn),
        }
    }
}

impl TryFrom<String> for AnswerIn {
    type Error = InvalidAnswerIn;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<AnswerIn> for String {
    fn from(value: AnswerIn) -> Self {
        value.to_string()
    }
}

impl fmt::Display for AnswerIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Any => f.write_str("any"),
            Self::Session(id) => write!(f, "session:{id}"),
            Self::Hand => f.write_str("hand"),
        }
    }
}

/// A question or an action that waits for the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenItem {
    /// Session name of the asker.
    pub asked_by: AuthorName,
    /// The options in full, empty for an action.
    pub options: Vec<String>,
    /// The 1-based recommended option.
    pub recommended: Option<usize>,
    /// Why that option is recommended.
    pub why: Option<String>,
    /// What waits meanwhile.
    pub blocked: String,
    /// Where the user can answer.
    pub answer_in: AnswerIn,
    /// The recorded answer, absent on a new item.
    pub answer: Option<ItemAnswer>,
    /// The closure, absent on a new item.
    pub closed: Option<ItemClosure>,
}

/// The user's recorded answer to an open item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemAnswer {
    /// The user's words.
    pub words: String,
    /// The session in which they typed the answer.
    pub session: SessionId,
    /// When they typed the answer.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The agent that recorded the answer.
    pub recorded_by: AuthorName,
}

/// The record of carrying an open item out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemClosure {
    /// When the item was closed.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The agent that closed it.
    pub by: AuthorName,
    /// The lasting decision, when there was one.
    pub decision: Option<EntryId>,
    /// What was done.
    pub note: String,
}

/// Where an open item stands, derived from its status and fields
/// (2_design.md §18.4). A trial entry without an `item` object reads
/// active as `open` and retired as `closed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemState {
    /// Active, with no recorded answer.
    Open,
    /// Active, with the user's answer recorded.
    Answered,
    /// Retired by `entry close` once carried out.
    Closed,
    /// Retired without a closure: the question went away or was wrong.
    Withdrawn,
}

impl ItemState {
    /// Every state's name, in lifecycle order.
    pub const NAMES: [&'static str; 4] = ["open", "answered", "closed", "withdrawn"];

    /// The state's name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Answered => "answered",
            Self::Closed => "closed",
            Self::Withdrawn => "withdrawn",
        }
    }

    /// The state of `entry`, or `None` when it is not filed under `open-item`.
    #[must_use]
    pub fn of(entry: &super::Entry) -> Option<Self> {
        use super::EntryStatus;
        if entry.tag.as_str() != OPEN_ITEM_TAG {
            return None;
        }
        let item = entry.item.as_ref();
        Some(match entry.status {
            EntryStatus::Active if item.is_some_and(|i| i.answer.is_some()) => Self::Answered,
            EntryStatus::Active => Self::Open,
            EntryStatus::Retired if item.is_none_or(|i| i.closed.is_some()) => Self::Closed,
            EntryStatus::Retired | EntryStatus::Superseded | EntryStatus::Forgotten => {
                Self::Withdrawn
            }
        })
    }

    /// Whether the item is still waiting to be carried out.
    #[must_use]
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Closed | Self::Withdrawn)
    }
}

impl fmt::Display for ItemState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An unknown open-item state name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("expected open, answered, closed or withdrawn")]
pub struct InvalidItemState;

impl FromStr for ItemState {
    type Err = InvalidItemState;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "open" => Ok(Self::Open),
            "answered" => Ok(Self::Answered),
            "closed" => Ok(Self::Closed),
            "withdrawn" => Ok(Self::Withdrawn),
            _ => Err(InvalidItemState),
        }
    }
}
