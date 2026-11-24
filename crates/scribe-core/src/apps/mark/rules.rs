//! The mark app's rules: a marker refused before it is handed to the agent
//! (docs/design/scribe/2_design.md §14.3). Pure: no I/O (STD-02 R5).

use crate::capture::Mark;
use crate::tag::TagTable;

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
