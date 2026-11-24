//! `marks.json`: each `#scribe` annotation marker's outcome, and the
//! locked write that records annotation entries with them
//! (docs/design/scribe/2_design.md §14.4, §14.5).

use super::Store;
use crate::annotation::MarkRecord;
use crate::entry::{Entry, NewEntry};
use crate::{Result, SessionId};

/// Which marker an update is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkKey {
    /// The session.
    pub session: SessionId,
    /// The marked turn's message uuid.
    pub message: String,
    /// The marked turn's `prompt_sha256`.
    pub prompt_sha256: String,
    /// The marker's number in the turn.
    pub marker: u32,
}

/// What to record for one marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkChange {
    /// Write this checked annotation entry (`content` the statement,
    /// `quote` his words, `stated_by` [`Author::annotation`]).
    Write(Box<NewEntry>),
    /// The model found nothing to record.
    Nothing,
    /// Refused with this code.
    Refused(&'static str),
    /// The model call failed: one more attempt spent.
    Failed,
}

/// What recording one marker came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkResult {
    /// The entry was written.
    Added(Entry),
    /// An earlier run already wrote it, or already settled the marker
    /// (`record` says how).
    Already(MarkRecord),
    /// An active entry of the same tag and project already says it:
    /// nothing written, refused `covered`.
    Covered(Entry),
    /// The quote holds text redaction would change: refused.
    NeedsRedaction,
    /// Recorded as nothing found.
    Nothing,
    /// Recorded as refused with this code.
    Refused(&'static str),
    /// The call failed; `attempts` spent so far, and whether that was the
    /// last ([`MAX_ATTEMPTS`]), so the marker is now refused `model_failed`.
    Failed {
        /// Attempts spent, this one included.
        attempts: u32,
        /// No attempt is left.
        gave_up: bool,
    },
}

pub use crate::annotation::MODEL_FAILED;
/// The refusal code of an annotation an active entry already covers.
pub const COVERED: &str = "covered";
/// The refusal code of a quote redaction would change.
pub const NEEDS_REDACTION: &str = "needs_redaction";

impl Store {
    /// Every marker outcome of `session`, in file order; none for a
    /// missing file or store. Reads only.
    pub(in crate::store) fn marks(&self, session: &SessionId) -> Result<Vec<MarkRecord>> {
        Ok(self
            .all_marks()?
            .into_iter()
            .filter(|m| m.session == *session)
            .collect())
    }

    /// The markers no entry or answer settled yet ([`MarkRecord::uncataloged`]):
    /// pending a retry or given up after [`MAX_ATTEMPTS`], of `session` or of
    /// every session, in file order. Reads only.
    pub(in crate::store) fn uncataloged_marks(
        &self,
        session: Option<&SessionId>,
    ) -> Result<Vec<MarkRecord>> {
        Ok(self
            .all_marks()?
            .into_iter()
            .filter(|m| m.uncataloged() && session.is_none_or(|s| m.session == *s))
            .collect())
    }
}
