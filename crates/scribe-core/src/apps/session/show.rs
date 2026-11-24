//! `scribe session show`. Read-only: no lock, no audit, no write.

use crate::SessionId;

/// What `session show` found.
pub enum Shown {
    /// The session record, with the handoffs that name it.
    Record {
        /// The record and what is read with it.
        view: Box<crate::SessionView>,
        /// The active handoffs that reference it, newest first.
        handoffs: Vec<crate::Entry>,
    },
    /// No record, only the handoffs that name the session.
    Handoffs {
        /// The session.
        id: SessionId,
        /// Its switch, when it has one.
        control: Option<crate::session::SessionControl>,
        /// The active handoffs that reference it, newest first.
        handoffs: Vec<crate::Entry>,
    },
}

/// `session show`: resolve `id` (a session or a handoff's session), read
/// the handoffs that name it, then its record. A session with handoffs but
/// no record shows the handoffs; `segment`, when given, must be in the
/// record's log.
///
/// # Errors
///
/// [`ReadError::SegmentNotFound`](crate::runtime::errors::ReadError),
/// an unknown or ambiguous id, or a store error.
pub fn show<E>(
    ctx: &crate::runtime::Ctx<'_, E>,
    id: &SessionId,
    segment: Option<std::num::NonZeroU32>,
) -> std::result::Result<Shown, crate::runtime::errors::ReadError<E>> {
    use crate::runtime::errors::ReadError;
    let id = ctx.store.read().sessions().resolve_or_handoff(id)?;
    let handoffs = crate::apps::entry::handoffs_for_session(&ctx.store.read(), &id)?;
    let view = match super::session_view(&ctx.store.read(), &id) {
        Ok(view) => view,
        Err(crate::Error::SessionNotFound { .. }) if !handoffs.is_empty() => {
            if let Some(n) = segment {
                return Err(ReadError::SegmentNotFound {
                    session: id,
                    n: n.get(),
                    count: 0,
                });
            }
            let control = ctx.store.read().sessions().control()?;
            let control = control.get(&id).cloned();
            return Ok(Shown::Handoffs {
                id,
                control,
                handoffs,
            });
        }
        Err(err) => return Err(err.into()),
    };
    if let Some(n) = segment
        && view.record.segment(n.get()).is_none()
    {
        return Err(ReadError::SegmentNotFound {
            session: view.record.id.clone(),
            n: n.get(),
            count: view.record.log.len(),
        });
    }
    Ok(Shown::Record {
        view: Box::new(view),
        handoffs,
    })
}
