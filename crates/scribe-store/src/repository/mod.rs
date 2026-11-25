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
//! A [`Snapshot`]'s repositories are [`ReadOnly`]: only their reads are
//! callable, so a holder of a read cannot write. A [`Tx`]'s are
//! [`ReadWrite`], and their writes join that transaction.
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
use std::marker::PhantomData;

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

/// What a repository may do: [`ReadOnly`] from a [`Snapshot`], [`ReadWrite`]
/// from a [`Tx`]. Sealed, so no other capability exists.
pub trait Access: sealed::Sealed + Clone + Copy + std::fmt::Debug {}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::ReadOnly {}
    impl Sealed for super::ReadWrite {}
}

/// The capability of a [`Snapshot`]'s repositories: their reads only. A
/// record write is not callable on them, so a caller handed a read cannot
/// start a write of its own outside the use case's transaction and audit.
///
/// ```compile_fail
/// # fn put(store: &scribe_store::Store, entry: &scribe_store::entry::Entry) {
/// store.read().entries().put(entry, true).unwrap();
/// # }
/// ```
///
/// ```compile_fail
/// # fn record(store: &scribe_store::Store, event: &scribe_store::AuditEvent) {
/// store.read().audit().record(event).unwrap();
/// # }
/// ```
///
/// The same writes compile on a [`Tx`]'s repositories, and its reads on
/// either:
///
/// ```
/// # fn put(
/// #     store: &scribe_store::Store,
/// #     entry: &scribe_store::entry::Entry,
/// #     event: &scribe_store::AuditEvent,
/// # ) {
/// let _ = store.read().entries().get(&entry.id);
/// let _: scribe_store::repository::Entries<'_> = store.read().entries();
/// let _ = store.write(|tx| tx.entries().put(entry, true));
/// let _ = store.write(|tx| tx.entries().get(&entry.id).map(drop));
/// let _ = store.write(|tx| tx.audit().record(event));
/// let _ = store.read().audit().events();
/// # }
/// ```
#[derive(Clone, Copy, Debug)]
pub enum ReadOnly {}

/// The capability of a [`Tx`]'s repositories: their reads and their writes,
/// which join the transaction [`Store::write`] holds.
#[derive(Clone, Copy, Debug)]
pub enum ReadWrite {}

impl Access for ReadOnly {}
impl Access for ReadWrite {}

macro_rules! repositories {
    ($($(#[$doc:meta])* $name:ident => $repo:ident;)*) => {
        impl<'s> Snapshot<'s> {
            $($(#[$doc])* #[must_use] pub fn $name(&self) -> $repo<'s, ReadOnly> {
                $repo { store: self.store, access: PhantomData }
            })*
        }
        impl<'s> Tx<'s> {
            $($(#[$doc])* #[must_use] pub fn $name(&self) -> $repo<'s, ReadWrite> {
                $repo { store: self.store, access: PhantomData }
            })*
        }
        $(
            $(#[$doc])*
            ///
            /// Its reads take any [`Access`]; its writes need [`ReadWrite`],
            /// the capability only a [`Tx`] gives.
            #[derive(Clone, Copy, Debug)]
            pub struct $repo<'s, A: Access = ReadOnly> {
                store: &'s Store,
                access: PhantomData<A>,
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
