//! Mark verbs: recording each `#scribe` marker's outcome (2_design.md
//! §14.5).

use crate::annotation::{MarkRecord, MarkState};
use crate::runtime::resolve::refuse_forgotten;
use crate::{Result, SessionId, Store};
use time::OffsetDateTime;

/// The mark verbs, implemented for [`Store`].
pub trait MarkVerbs {
    /// Record each marker's outcome in one write (2_design.md §14.5).
    ///
    /// A marker already settled in `marks.json` changes nothing and comes
    /// back [`MarkResult::Already`].
    fn record_marks(
        &self,
        updates: Vec<(MarkKey, MarkChange)>,
        now: OffsetDateTime,
    ) -> Result<Vec<MarkResult>>;
}

impl MarkVerbs for Store {
    fn record_marks(
        &self,
        updates: Vec<(MarkKey, MarkChange)>,
        now: OffsetDateTime,
    ) -> Result<Vec<MarkResult>> {
        if updates.is_empty() {
            return Ok(Vec::new());
        }
        self.write(|tx| {
            let mut marks = tx.marks().all()?;
            let mut out = Vec::with_capacity(updates.len());
            let mut changed = false;
            for (key, change) in updates {
                refuse_forgotten(self, &key.session)?;
                let at = marks
                    .iter()
                    .position(|m| m.session == key.session && m.is(&key.message, key.marker));
                if let Some(found) = at.map(|i| &marks[i])
                    && found.outcome.is_final()
                {
                    out.push(MarkResult::Already(found.clone()));
                    continue;
                }
                let MarkChange::Refused(code) = change;
                let next = MarkRecord {
                    session: key.session.clone(),
                    message: key.message.clone(),
                    prompt_sha256: key.prompt_sha256.clone(),
                    marker: key.marker,
                    outcome: MarkState::Refused,
                    entry: None,
                    code: Some(code.to_owned()),
                    attempts: at.map_or(0, |i| marks[i].attempts),
                    at: now,
                };
                match at {
                    Some(i) => marks[i] = next,
                    None => marks.push(next),
                }
                changed = true;
                out.push(MarkResult::Refused(code));
            }
            if changed {
                tx.marks().put_all(&marks)?;
            }
            Ok(out)
        })
    }
}

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
