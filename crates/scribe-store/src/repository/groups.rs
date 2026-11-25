//! The group repository: session groups, their seats and their rows.

use super::{Access, ReadWrite};
use super::{GroupRows, Groups, RowError};
use crate::LastGroup;
use crate::error::Result;
use crate::group::Group;
use crate::message::MessageSearch;
use crate::{Project, SessionId};

impl<A: Access> Groups<'_, A> {
    /// Every group, newest first, with seats and their active handoffs.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn all(&self) -> Result<Vec<Group>> {
        self.store.groups()
    }

    /// One group by exact id or unique prefix.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn get(&self, id: &str) -> Result<Group> {
        self.store.group(id)
    }

    /// The number of the group an open project slug, an exact id or a
    /// unique id prefix names.
    ///
    /// # Errors
    ///
    /// `group_not_found` naming the place searched, `id_ambiguous`, and the
    /// store's read errors.
    pub fn number(&self, input: &str) -> Result<i64> {
        self.store.group_number(input)
    }

    /// Run `f` over the group rows in one read; `T::default()` before the
    /// store has groups. `f` may run twice, the second time on a read-only
    /// copy of the database.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn read_rows<T: Default>(
        &self,
        f: impl Fn(&GroupRows<'_>) -> std::result::Result<T, RowError>,
    ) -> Result<T> {
        self.store.read_group_rows(f)
    }

    /// [`Groups::read_rows`] inside one read transaction, so its reads see
    /// one state: the read a group close takes its evidence from. `action`
    /// names the read in a database error.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn read_rows_in_tx<T: Default>(
        &self,
        action: &'static str,
        f: impl Fn(&GroupRows<'_>) -> std::result::Result<T, RowError>,
    ) -> Result<T> {
        self.store.read_group_rows_in_tx(action, f)
    }

    /// Whether a deferred ancestry check is pending for `session`.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn deferred_ancestry(&self, session: &SessionId) -> Result<bool> {
        self.store.deferred_ancestry(session)
    }

    /// The most recently opened group in `project`. Reads only.
    pub fn last(&self, project: &Project) -> Result<Option<LastGroup>> {
        self.store.last_group(project)
    }

    /// The stored messages of `group`'s member sessions matching `tokens`.
    /// Reads only.
    pub fn search_messages(&self, group: &str, tokens: &[String]) -> Result<MessageSearch> {
        self.store.search_group_messages(group, tokens)
    }
}

/// The writes, on the repository a [`Tx`](super::Tx) gives: a
/// [`Snapshot`](super::Snapshot)'s cannot write.
impl Groups<'_, ReadWrite> {
    /// Run `f` over the group rows in one write step, `action` naming the
    /// write in a database error (`"join group in"`), part of the
    /// [`Tx`](super::Tx)'s write.
    ///
    /// # Errors
    ///
    /// `f`'s error, and the store's write errors.
    pub fn rows<T>(
        &self,
        action: &'static str,
        f: impl FnOnce(&GroupRows<'_>) -> Result<T>,
    ) -> Result<T> {
        self.store.group_rows(action, f)
    }
}
