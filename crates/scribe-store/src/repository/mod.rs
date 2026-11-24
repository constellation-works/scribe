//! The store's access surface (ORB-27707 step 2): [`Store::read`] gives a
//! [`Snapshot`] and [`Store::write`] runs a closure with a [`Tx`], and each
//! exposes one repository per record kind over `scribe.db`.
//!
//! A repository holds no rule: it reads and writes records as they are.
//! Validation is the caller's, run inside the `write` closure so that a check
//! and its write share one transaction. The use-case verbs
//! still on [`Store`] move onto this surface in the steps that follow, and
//! are then removed.
//!
//! One file per record holds its repository and the [`Store`] reads and
//! writes under it: `entries`, `sessions`, `proposals`, `tags`, `groups`,
//! `memory`, `messages`, `marks`, `pickups` and `audit`; `ids` resolves an
//! exact id or a unique prefix for any of them.
//!
//! A [`Snapshot`] reads as each read runs: every repository call opens its
//! own read of the database (or, inside a write, sees that write's pending
//! rows), as the store's reads always have. It does not pin one read
//! transaction across calls.

pub(crate) mod audit;
pub(crate) mod entries;
pub(crate) mod groups;
pub(crate) mod ids;
pub(crate) mod marks;
pub(crate) mod memory;
pub(crate) mod messages;
pub(crate) mod pickups;
pub(crate) mod proposals;
pub(crate) mod sessions;
pub(crate) mod tags;

use crate::Store;
pub use crate::db::{CaptureState, ClaudeSeenIds, CloseHeader, GroupRows, MemberWindow, RowError};
pub use crate::db::{FileOp, HistoryRow, MemoryCommit, PendingRow, Revision};
use crate::error::{Error, Result};
pub use memory::{MemoryFiles, MemoryJournal, MemoryRows};

impl Store {
    /// A read-only view of the store: no lock and no write.
    #[must_use]
    pub fn read(&self) -> Snapshot<'_> {
        Snapshot { store: self }
    }

    /// Run `f` as one store write with a [`Tx`] for its reads and writes: the
    /// store's lock and `BEGIN IMMEDIATE` are taken before `f` runs, inside
    /// the command's transaction when there is one ([`Store::transaction`]),
    /// and `f`'s writes commit when it returns `Ok`; an `Err` rolls back
    /// everything `f` wrote. No model call may run inside.
    ///
    /// # Errors
    ///
    /// `f`'s error, and [`Store::transaction`]'s.
    pub fn write<T, E: From<Error>>(
        &self,
        f: impl FnOnce(&Tx<'_>) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        self.step(|| f(&Tx { store: self }))
    }

    /// [`Store::write`] that keeps its committed result when the cleanup after
    /// the commit fails, and returns that cleanup's outcome beside it: the
    /// store's write-with-cleanup step. Run it outside a pending write, so
    /// the commit is its own.
    ///
    /// # Errors
    ///
    /// `f`'s error, and [`Store::transaction`]'s.
    pub fn write_with_cleanup<T>(
        &self,
        f: impl FnOnce(&Tx<'_>) -> Result<T>,
    ) -> Result<(T, Result<()>)> {
        self.write_step_with_cleanup(|| f(&Tx { store: self }))
    }
}

/// A read of the store ([`Store::read`]).
#[derive(Clone, Copy, Debug)]
pub struct Snapshot<'s> {
    store: &'s Store,
}

impl Snapshot<'_> {
    /// Whether the data root exists yet; nothing is created.
    ///
    /// # Errors
    ///
    /// The privacy errors for a root that is a link, another user's or open
    /// to others.
    pub fn exists(&self) -> Result<bool> {
        self.store.root_exists()
    }
}

/// The reads and writes of one store transaction ([`Store::write`]). Its
/// reads see the transaction's own pending writes.
#[derive(Clone, Copy, Debug)]
pub struct Tx<'s> {
    store: &'s Store,
}

macro_rules! repositories {
    ($($(#[$doc:meta])* $name:ident => $repo:ident;)*) => {
        impl<'s> Snapshot<'s> {
            $($(#[$doc])* #[must_use] pub fn $name(&self) -> $repo<'s> { $repo { store: self.store } })*
        }
        impl<'s> Tx<'s> {
            $($(#[$doc])* #[must_use] pub fn $name(&self) -> $repo<'s> { $repo { store: self.store } })*
        }
        $(
            $(#[$doc])*
            #[derive(Clone, Copy, Debug)]
            pub struct $repo<'s> {
                store: &'s Store,
            }
        )*
    };
}

repositories! {
    /// Entries: notes, findings, handoffs, decisions and open items.
    entries => Entries;
    /// Sessions: their records, `s-<n>` numbers and on/off control state.
    sessions => Sessions;
    /// Proposals and claims.
    proposals => Proposals;
    /// The config and the store tags.
    tags => Tags;
    /// Session groups and their seats.
    groups => Groups;
    /// The descriptive memory.
    memory => Memory;
    /// Stored session messages.
    messages => Messages;
    /// `#scribe` marker outcomes.
    marks => Marks;
    /// Consumer pickups of action entries.
    pickups => Pickups;
    /// The audit log.
    audit => Audit;
}
