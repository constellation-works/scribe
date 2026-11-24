//! `marks.json`: each `#scribe` annotation marker's outcome
//! (docs/design/scribe/2_design.md §14.5).

use super::Store;
use crate::annotation::MarkRecord;
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
    /// Refused with this code (`agent_handoff` for one handed to the agent).
    Refused(&'static str),
}

/// What recording one marker came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkResult {
    /// An earlier run already settled the marker (`record` says how).
    Already(MarkRecord),
    /// Recorded as refused with this code.
    Refused(&'static str),
}

pub use crate::annotation::{AGENT_HANDOFF, MODEL_FAILED};

impl Store {
    /// Every marker outcome of `session`, in file order; none for a
    /// missing file or store. Reads only.
    pub(crate) fn marks(&self, session: &SessionId) -> Result<Vec<MarkRecord>> {
        Ok(self
            .all_marks()?
            .into_iter()
            .filter(|m| m.session == *session)
            .collect())
    }

    /// The markers no entry or answer settled yet ([`MarkRecord::uncataloged`]):
    /// pending a retry or given up after [`MAX_ATTEMPTS`], of `session` or of
    /// every session, in file order. Reads only.
    pub(crate) fn uncataloged_marks(&self, session: Option<&SessionId>) -> Result<Vec<MarkRecord>> {
        Ok(self
            .all_marks()?
            .into_iter()
            .filter(|m| m.uncataloged() && session.is_none_or(|s| m.session == *s))
            .collect())
    }
}
