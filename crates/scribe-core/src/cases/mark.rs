//! Mark verbs: recording each `#scribe` marker's outcome (2_design.md
//! §14.5).

use crate::annotation::{MarkRecord, MarkState};
use crate::cases::session::SessionVerbs;
use crate::{MarkChange, MarkKey, MarkResult, Result, SessionId, Store};
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
                self.refuse_forgotten(&key.session)?;
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

/// `mark list`: the markers scribe wrote no entry for (pending, given up
/// or handed to the agent), of `session` when
/// given, resolved across the store and the `provider` transcripts on disk
/// (an unknown id is kept as typed). The session as resolved, and the
/// markers.
///
/// # Errors
///
/// An ambiguous or too-short prefix, or a store error.
pub fn list<E: crate::cases::PortError>(
    ctx: &crate::cases::Ctx<'_, E>,
    provider: crate::Provider,
    session: Option<&SessionId>,
) -> Result<(Option<SessionId>, Vec<MarkRecord>)> {
    let session = session
        .map(|id| crate::cases::resolve::session_or_typed(ctx, provider, id))
        .transpose()?;
    let marks = ctx.store.read().marks().uncataloged(session.as_ref())?;
    Ok((session, marks))
}
