//! `#scribe` as an annotation (docs/design/scribe/2_design.md §14): a
//! marker anywhere in a sentence the user typed, handed at the end of the
//! same turn to the agent, which records the entry with their verbatim words
//! (ORB-27911: scribe calls no model for it).
//!
//! This module is pure: [`marks`] finds the markers of a turn and the
//! mentions that are not markers (§14.1), [`record_tag_refusal`] refuses a
//! marker naming a typed record, and [`MarkRecord`] is a marker's outcome
//! as `marks.json` keeps it. The prompt hook and the `Stop` hook do the I/O.
//!
//! A line that starts with `#scribe` is a marker too (§14.6): forms 1 to 3
//! fold into it, so `#scribe idea: …` hints `idea` and `#scribe add this to
//! preferences` hints `preferences`. Only a whole-line settling command
//! (form 4, `crate::capture::scan`) is not a marker.
//!
//! No I/O (STD-02 R5).

pub use crate::capture::{MAX_MARKS, Mark, MarkProblem, marks, turn_marks};
use crate::tag::TagTable;

/// The most model calls one marked turn got across `Stop`s, when scribe
/// still called a model for markers: an older `pending` record holds fewer.
pub const MAX_ATTEMPTS: u32 = 3;

/// Refuse a marker explicitly naming a typed record before handing it over.
#[must_use]
pub fn record_tag_refusal(mark: &Mark, tags: &TagTable) -> Option<CheckRefusal> {
    mark.hint(tags)
        .filter(|tag| crate::entry::is_decision_or_item(tag.as_str()))
        .map(|_| CheckRefusal::RecordTag)
}

/// Why a marker is refused before it is handed over (§14.3). It is recorded
/// `refused` with its code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckRefusal {
    /// The marker names a decision or open item, written through entry add.
    RecordTag,
}

impl CheckRefusal {
    /// The refusal code it is recorded and audited with.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::RecordTag => "unknown_tag",
        }
    }

    /// Why, for the user's echo.
    #[must_use]
    pub fn message(&self) -> &'static str {
        match self {
            Self::RecordTag => "record decisions and open items through scribe entry add",
        }
    }
}

/// Where one marker stands in `marks.json` (2_design.md §14.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkState {
    /// An entry was written for it (or an earlier one already was).
    Entry,
    /// The model found nothing to record.
    None,
    /// Refused, with its code (`agent_handoff` for one handed to the
    /// agent): settled.
    Refused,
    /// The model call failed; the next `Stop` tries again.
    Pending,
}

impl MarkState {
    /// The canonical lowercase token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::None => "none",
            Self::Refused => "refused",
            Self::Pending => "pending",
        }
    }

    /// Whether the marker is settled: anything but `pending`.
    #[must_use]
    pub fn is_final(self) -> bool {
        self != Self::Pending
    }
}

/// One marker's outcome as `marks.json` keeps it: ids and codes only,
/// never words (2_design.md §14.5).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkRecord {
    /// The session the marked turn is in.
    pub session: crate::SessionId,
    /// The marked turn's message uuid.
    pub message: String,
    /// The marked turn's `prompt_sha256`.
    pub prompt_sha256: String,
    /// The marker's number in the turn, from 1.
    pub marker: u32,
    /// Where it stands.
    pub outcome: MarkState,
    /// For `entry`, the entry's id; `null` otherwise.
    pub entry: Option<crate::EntryId>,
    /// For `refused`, the refusal code; `null` otherwise.
    pub code: Option<String>,
    /// Model calls made for it so far.
    pub attempts: u32,
    /// When the outcome was last written.
    #[serde(with = "time::serde::rfc3339")]
    pub at: time::OffsetDateTime,
}

impl MarkRecord {
    /// Whether the fields agree with the outcome.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.marker >= 1
            && !self.message.is_empty()
            && match self.outcome {
                MarkState::Entry => self.entry.is_some() && self.code.is_none(),
                MarkState::Refused => {
                    self.entry.is_none() && self.code.as_deref().is_some_and(|c| !c.is_empty())
                }
                MarkState::None => self.entry.is_none() && self.code.is_none(),
                MarkState::Pending => {
                    self.entry.is_none() && self.code.is_none() && self.attempts < MAX_ATTEMPTS
                }
            }
    }

    /// Whether it is the marker `marker` of message `message`.
    #[must_use]
    pub fn is(&self, message: &str, marker: u32) -> bool {
        self.message == message && self.marker == marker
    }

    /// Whether every call for it failed: refused `model_failed` after
    /// [`MAX_ATTEMPTS`] (§14.5), when scribe still called a model.
    #[must_use]
    pub fn gave_up(&self) -> bool {
        self.outcome == MarkState::Refused && self.code.as_deref() == Some(MODEL_FAILED)
    }

    /// Whether it was handed to the agent to record (refused
    /// [`AGENT_HANDOFF`]).
    #[must_use]
    pub fn handed_off(&self) -> bool {
        self.outcome == MarkState::Refused && self.code.as_deref() == Some(AGENT_HANDOFF)
    }

    /// Whether scribe itself wrote no entry and no answer for it: pending
    /// from before ORB-27911 (the next `Stop` hands it over), given up
    /// ([`MarkRecord::gave_up`]) or handed to the agent
    /// ([`MarkRecord::handed_off`]). `scribe mark list` lists these.
    #[must_use]
    pub fn uncataloged(&self) -> bool {
        self.outcome == MarkState::Pending || self.gave_up() || self.handed_off()
    }
}

/// The refusal code of a marker whose model calls all failed, before
/// ORB-27911.
pub const MODEL_FAILED: &str = "model_failed";

/// The code of a marker handed to the agent to record (ORB-27911).
pub const AGENT_HANDOFF: &str = "agent_handoff";
