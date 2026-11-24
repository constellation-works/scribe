//! `marks.json`: each `#scribe` annotation marker's outcome
//! (docs/design/scribe/2_design.md §14.5).

use super::Store;
use crate::annotation::MarkRecord;
use crate::{Result, SessionId};

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
