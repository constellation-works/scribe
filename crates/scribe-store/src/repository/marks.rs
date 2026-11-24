//! `marks.json`: each `#scribe` annotation marker's outcome
//! (docs/design/scribe/2_design.md §14.5).

use super::Marks;
use crate::MarkRecord;
use crate::db::{self, docs};
use crate::error::Result;
use crate::{SessionId, Store};

impl Marks<'_> {
    /// Every marker outcome of `session`, in file order.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn of_session(&self, session: &SessionId) -> Result<Vec<MarkRecord>> {
        self.store.marks(session)
    }

    /// Every marker outcome, in file order.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn all(&self) -> Result<Vec<MarkRecord>> {
        self.store.all_marks()
    }

    /// Replace the marks.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put_all(&self, marks: &[MarkRecord]) -> Result<()> {
        self.store.write_marks(marks)
    }

    /// The markers no entry or answer settled yet, of `session` or of every
    /// session, in file order. Reads only.
    pub fn uncataloged(&self, session: Option<&SessionId>) -> Result<Vec<MarkRecord>> {
        self.store.uncataloged_marks(session)
    }
}

impl Store {
    /// Every marker outcome, in order.
    pub(crate) fn all_marks(&self) -> Result<Vec<MarkRecord>> {
        self.read_or_empty(docs::marks)
    }

    /// Replace the marks, in the write under way.
    pub(crate) fn write_marks(&self, marks: &[MarkRecord]) -> Result<()> {
        self.with_conn(|conn, path| {
            docs::put_marks(conn, path, marks, |e| {
                db::db_error("write marks to", path, e)
            })
        })
    }
}

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
