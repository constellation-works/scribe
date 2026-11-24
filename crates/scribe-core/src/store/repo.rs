//! The store's access surface (ORB-27707 step 2): [`Store::read`] gives a
//! [`Snapshot`] and [`Store::write`] runs a closure with a [`Tx`], and each
//! exposes one repository per record kind over `scribe.db`.
//!
//! A repository holds no rule: it reads and writes records as they are.
//! Validation is the caller's, run inside the `write` closure so that a check
//! and its write share one transaction (`crate::checks`). The use-case verbs
//! still on [`Store`] move onto this surface in the steps that follow, and
//! are then removed.
//!
//! A [`Snapshot`] reads as each read runs: every repository call opens its
//! own read of the database (or, inside a write, sees that write's pending
//! rows), as the store's reads always have. It does not pin one read
//! transaction across calls.

use super::audit::AuditEvent;
use super::{
    EntryList, MemoryShown, MemoryView, Redactor, SessionList, SessionView, Skipped, Store,
};
use crate::action::Pickup;
use crate::annotation::MarkRecord;
use crate::error::{Error, Result};
use crate::group::Group;
use crate::message::{MessageSearch, NewMessage};
use crate::{
    Config, ControlState, Entry, EntryFilter, EntryId, Proposal, SessionFilter, SessionId,
    SessionRecord, StoreTag,
};
use std::collections::BTreeMap;
use std::num::NonZeroU64;
use time::OffsetDateTime;

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

impl Entries<'_> {
    /// The entry an exact id or a unique prefix of at least four characters
    /// names.
    ///
    /// # Errors
    ///
    /// `invalid_id` for input that is no id or prefix, `id_ambiguous`,
    /// [`Error::EntryNotFound`] or `entry_not_found` naming the place
    /// searched, and the store's read errors.
    pub fn resolve(&self, input: &str) -> Result<Entry> {
        self.store.find_entry(input)
    }

    /// Where the entry `id` is kept, for an error to name.
    #[must_use]
    pub fn path(&self, id: &EntryId) -> std::path::PathBuf {
        self.store.entry_path(id)
    }

    /// Erase `entry` from the stored messages it cites and from the archive
    /// tables, in the write under way (`proposals`: the claims before the
    /// forget).
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn forget_residue(
        &self,
        entry: &Entry,
        proposals: &[Proposal],
        now: OffsetDateTime,
    ) -> Result<()> {
        self.store.forget_entry_residue(entry, proposals, now)
    }

    /// Every entry, plus the rows that did not read.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn all(&self) -> Result<(Vec<Entry>, Vec<Skipped>)> {
        self.store.load_entries()
    }

    /// The entry with this id.
    ///
    /// # Errors
    ///
    /// [`Error::EntryNotFound`], and the store's read errors.
    pub fn get(&self, id: &EntryId) -> Result<Entry> {
        self.store.entry(id)
    }

    /// The entries `filter` selects ([`Store::list_entries`]).
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn list(&self, filter: &EntryFilter) -> Result<EntryList> {
        self.store.list_entries(filter)
    }

    /// Write `entry` in the transaction under way; `new` refuses to replace
    /// an existing row.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put(&self, entry: &Entry, new: bool) -> Result<()> {
        self.store.write_entry(entry, new)
    }
}

impl Sessions<'_> {
    /// One session's record and its log ([`Store::session`]).
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn get(&self, id: &SessionId) -> Result<SessionView> {
        self.store.session(id)
    }

    /// The sessions `filter` selects ([`Store::list_sessions`]).
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn list(&self, filter: &SessionFilter) -> Result<SessionList> {
        self.store.list_sessions(filter)
    }

    /// Every session's `s-<n>`.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn numbers(&self) -> Result<BTreeMap<SessionId, NonZeroU64>> {
        self.store.session_numbers()
    }

    /// Every switch and Codex cursor.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn control(&self) -> Result<ControlState> {
        self.store.control()
    }

    /// Write `record` whole in the transaction under way.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put(&self, record: &SessionRecord) -> Result<()> {
        self.store
            .sessions_tx(|tx| tx.put_record(record).map(|_| ()))
    }

    /// Write the switches and cursors that changed from `before` to
    /// `after`.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put_control(&self, before: &ControlState, after: &ControlState) -> Result<()> {
        self.store.sessions_tx(|tx| tx.put_control(before, after))
    }

    /// Keep a rebuild checkpoint of `id`; `false` when it has no row.
    ///
    /// # Errors
    ///
    /// A forgotten session's refusal, and the store's write errors.
    pub fn put_checkpoint(&self, id: &SessionId, text: &str, now: OffsetDateTime) -> Result<bool> {
        self.store
            .sessions_tx(|tx| tx.put_checkpoint(id, text, now))
    }

    /// Drop the rebuild checkpoint of `id`.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn drop_checkpoint(&self, id: &SessionId) -> Result<()> {
        self.store.sessions_tx(|tx| tx.drop_checkpoint(id))
    }
}

impl Proposals<'_> {
    /// The proposal an exact id or a unique prefix of at least four
    /// characters names.
    ///
    /// # Errors
    ///
    /// `invalid_id`, `id_ambiguous`, `proposal_not_found` naming the file
    /// searched, and the store's read errors.
    pub fn resolve(&self, input: &str) -> Result<Proposal> {
        self.store.find_proposal(input)
    }

    /// The index in `ids` of the proposal `input` names, as
    /// [`Proposals::resolve`] finds it.
    ///
    /// # Errors
    ///
    /// As [`Proposals::resolve`].
    pub fn position(&self, input: &str, ids: &[String]) -> Result<usize> {
        self.store.proposal_index(input, ids)
    }

    /// Where the proposals are kept, for an error to name.
    #[must_use]
    pub fn path(&self) -> std::path::PathBuf {
        self.store.root.join(super::PROPOSALS_FILE)
    }

    /// The entry ids a new entry may not take: those reserved by candidate
    /// proposals and those of deleted handoffs.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn reserved_entry_ids(&self) -> Result<Vec<EntryId>> {
        self.store.candidate_reserved_ids()
    }

    /// Every proposal, by id.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn all(&self) -> Result<Vec<Proposal>> {
        self.store.proposals()
    }

    /// Write the proposals that changed against the stored ones.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put_all(&self, proposals: &[Proposal]) -> Result<()> {
        self.store.write_proposals(proposals)
    }
}

impl Tags<'_> {
    /// The config, with the store tags merged in.
    ///
    /// # Errors
    ///
    /// The config's and the store's read errors.
    pub fn config(&self) -> Result<Config> {
        self.store.config()
    }

    /// The store tags (`scribe tag add`).
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn all(&self) -> Result<Vec<StoreTag>> {
        self.store.store_tags()
    }

    /// Replace the store tags.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put_all(&self, tags: &[StoreTag]) -> Result<()> {
        self.store.write_tags(tags)
    }
}

impl Groups<'_> {
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
}

impl Memory<'_> {
    /// The memory's profile, `now` items and milestones.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn view(&self) -> Result<MemoryView> {
        self.store.memory_view()
    }

    /// One memory file by its id.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn file(&self, id: &str) -> Result<MemoryShown> {
        self.store.memory_file(id)
    }
}

impl Messages<'_> {
    /// Stored messages matching `tokens`.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn search(&self, tokens: &[String]) -> Result<MessageSearch> {
        self.store.search_messages(tokens)
    }

    /// Store `messages`, masked by `redactor`; returns how many rows
    /// changed.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put(&self, messages: &[NewMessage], redactor: &Redactor) -> Result<usize> {
        self.store.write_messages(messages, redactor)
    }
}

impl Marks<'_> {
    /// Every marker outcome of `session`, in file order.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn of_session(&self, session: &SessionId) -> Result<Vec<MarkRecord>> {
        self.store.marks(session)
    }

    /// Replace the marks.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put_all(&self, marks: &[MarkRecord]) -> Result<()> {
        self.store.write_marks(marks)
    }
}

impl Pickups<'_> {
    /// Every pickup, oldest first.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn all(&self) -> Result<Vec<Pickup>> {
        self.store.pickups()
    }

    /// Replace the pickups.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put_all(&self, pickups: &[Pickup]) -> Result<()> {
        self.store.write_pickups(pickups)
    }
}

impl Audit<'_> {
    /// Every audit event and how many lines did not read.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn events(&self) -> Result<(Vec<AuditEvent>, usize)> {
        self.store.audit_events()
    }

    /// Record `event`, committing with the transaction under way.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn record(&self, event: &AuditEvent) -> Result<()> {
        self.store.record_audit(event)
    }
}
