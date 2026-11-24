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

use super::TagList;
use super::audit::{self, AuditEvent};
use super::db::{self, docs};
pub use super::db::{CaptureState, CloseHeader, GroupRows, MemberWindow, RowError};
pub use super::db::{FileOp, HistoryRow, MemoryCommit, PendingRow, Revision};
use super::memory::ProfileRevision;
pub use super::memory::{MemoryFiles, MemoryJournal, MemoryRows};
use super::{
    EntryList, LoadedSessions, MemoryShown, MemoryView, MemoryWritten, PassMemory, Redactor,
    SessionErasure, SessionList, SessionView, Skipped, Store,
};
use crate::action::Pickup;
use crate::annotation::MarkRecord;
use crate::brief::LastGroup;
use crate::error::{Error, Result};
use crate::group::Group;
use crate::message::{MessageSearch, NewMessage};
use crate::models::{MemoryContext, PendingContext};
use crate::{
    Config, ControlState, Entry, EntryFilter, EntryId, Proposal, SessionFilter, SessionId,
    SessionRecord, StoreTag,
};
use crate::{Project, Related, TagDef, TagName};
use std::collections::{BTreeMap, BTreeSet};
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

    /// Entries matching `filter` among those of the `members` sessions, when
    /// given. Reads only.
    pub fn list_scoped(
        &self,
        filter: &EntryFilter,
        members: Option<&BTreeSet<SessionId>>,
    ) -> Result<EntryList> {
        self.store.list_entries_scoped(filter, members)
    }

    /// Active `handoff` entries referencing `session:<id>`, newest first.
    /// Reads only.
    pub fn handoffs_for_session(&self, id: &SessionId) -> Result<Vec<Entry>> {
        self.store.handoffs_for_session(id)
    }

    /// The entries already recorded from one prompt, by their cite's
    /// `session` and `prompt_sha256`. Reads only.
    pub fn from_prompt(&self, session: &SessionId, prompt_sha256: &str) -> Result<Vec<Entry>> {
        self.store.entries_from_prompt(session, prompt_sha256)
    }
}

impl Sessions<'_> {
    /// Every session scribe knows: records, unreadable rows, numbers,
    /// switches and tombstones.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn loaded(&self) -> Result<LoadedSessions> {
        self.store.load_sessions()
    }

    /// Where the sessions are kept, for a not-found error to name.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn place(&self) -> Result<std::path::PathBuf> {
        self.store.sessions_place()
    }

    /// The database file, for an error to name.
    #[must_use]
    pub fn db_path(&self) -> std::path::PathBuf {
        self.store.root.join(super::DB_FILE)
    }

    /// The rebuild checkpoint of `original`'s session, if any.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn checkpoint(&self, original: &SessionRecord) -> Result<Option<SessionRecord>> {
        self.store.checkpoint_of(original)
    }

    /// Keep `record` as its session's rebuild checkpoint; `false` when the
    /// session has no row.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn keep_checkpoint(&self, record: &SessionRecord) -> Result<bool> {
        self.store.keep_checkpoint(record)
    }

    /// Write a rebuilt `record` and drop its checkpoint, together.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put_rebuilt(&self, record: &SessionRecord) -> Result<()> {
        self.store.put_rebuilt(record)
    }

    /// Change the switches and write a record in one session transaction:
    /// `f` changes the state and returns the record to write, if any. The
    /// switches are written only when they changed.
    ///
    /// # Errors
    ///
    /// `f`'s error, and the store's write errors.
    pub fn change<T>(
        &self,
        f: impl FnOnce(&mut ControlState) -> Result<(T, Option<SessionRecord>)>,
    ) -> Result<T> {
        self.store.change_sessions(f)
    }

    /// Commit one pass window in one transaction: `record`, its `messages`,
    /// the memory planned from `memory` (drafts already checked, with the
    /// `sensitive` refusals the check made, reported first), and the
    /// switches as `change` leaves them.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn commit_window(
        &self,
        record: Option<&SessionRecord>,
        messages: &[NewMessage],
        memory: Option<&PassMemory>,
        sensitive: &[scribe_domain::memory::Refusal],
        redactor: &Redactor,
        change: impl FnOnce(&mut ControlState),
    ) -> Result<MemoryWritten> {
        self.store
            .commit_window(record, messages, memory, sensitive, redactor, change)
    }

    /// Erase the session's rows, archives, documents and memory; `number`
    /// is its `s-<n>`, if it has one.
    ///
    /// # Errors
    ///
    /// [`Error::SessionNotFound`] when it has no row, and the store's write
    /// errors.
    pub fn erase(
        &self,
        id: &SessionId,
        number: Option<NonZeroU64>,
        now: OffsetDateTime,
    ) -> Result<SessionErasure> {
        self.store.erase_session(id, number, now)
    }

    /// The session an id, a unique prefix or an `s-<n>` names, also counting
    /// sessions known only through a handoff.
    ///
    /// # Errors
    ///
    /// [`Error::SessionNotFound`], `id_ambiguous`, and the store's read
    /// errors.
    pub fn resolve_or_handoff(&self, input: &SessionId) -> Result<SessionId> {
        self.store.resolve_session_or_handoff(input)
    }

    /// Set the session's `field` to `value`; returns how many rows changed,
    /// 0 when the session has no record or was forgotten.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put_field(
        &self,
        id: &SessionId,
        field: crate::session::SessionField,
        value: &str,
    ) -> Result<usize> {
        self.store.set_session_field(id, field, value)
    }

    /// The ids of the forgotten sessions.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn forgotten_ids(&self) -> Result<Vec<SessionId>> {
        Ok(super::db::sessions::read_forgotten(&self.store.root)?
            .map(|forgotten| forgotten.into_keys().collect())
            .unwrap_or_default())
    }

    /// Whether a group seats `id`.
    ///
    /// # Errors
    ///
    /// The store's errors.
    pub fn has_group_membership(&self, id: &SessionId) -> Result<bool> {
        self.store.has_group_membership(id)
    }

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

    /// The full id of the one session `input` names, or whose id starts with
    /// it. Reads only.
    pub fn resolve(&self, input: &SessionId) -> Result<SessionId> {
        self.store.resolve_session(input)
    }

    /// [`Sessions::resolve_or_handoff`], returning an id matching no known
    /// session as typed. Reads only.
    pub fn resolve_or_typed(&self, input: &SessionId) -> Result<SessionId> {
        self.store.resolve_session_or_typed(input)
    }

    /// [`Sessions::resolve_or_typed`], also counting `extra_ids` in the
    /// session id universe. Reads only.
    pub fn resolve_or_typed_with(
        &self,
        input: &SessionId,
        extra_ids: &[String],
    ) -> Result<SessionId> {
        self.store.resolve_session_or_typed_with(input, extra_ids)
    }

    /// Every session id the store knows: the universe display prefixes must
    /// be unique in. Reads only.
    pub fn ids(&self) -> Result<Vec<String>> {
        self.store.session_ids()
    }

    /// `related` with a `session:` id prefix replaced by the full id
    ///. Reads only.
    pub fn resolve_related(&self, related: &Related) -> Result<Related> {
        self.store.resolve_related(related)
    }

    /// [`Sessions::resolve_related`], also counting `extra_ids` in the
    /// session id universe. Reads only.
    pub fn resolve_related_with(&self, related: &Related, extra_ids: &[String]) -> Result<Related> {
        self.store.resolve_related_with(related, extra_ids)
    }

    /// [`Sessions::resolve_related`] over a list. Reads only.
    pub fn resolve_relateds(&self, related: &[Related]) -> Result<Vec<Related>> {
        self.store.resolve_relateds(related)
    }

    /// [`Sessions::resolve_related_with`] over a list. Reads only.
    pub fn resolve_relateds_with(
        &self,
        related: &[Related],
        extra_ids: &[String],
    ) -> Result<Vec<Related>> {
        self.store.resolve_relateds_with(related, extra_ids)
    }

    /// Session records matching `filter` among the `members` sessions, when
    /// given. Reads only.
    pub fn list_scoped(
        &self,
        filter: &SessionFilter,
        members: Option<&BTreeSet<SessionId>>,
    ) -> Result<SessionList> {
        self.store.list_sessions_scoped(filter, members)
    }

    /// Whether `#scribe forget` erased `id`. Reads only.
    pub fn is_forgotten(&self, id: &SessionId) -> Result<bool> {
        super::db::sessions::read_is_forgotten(&self.store.root, id)
    }

    /// Change the on/off state in the write under way: `change` gets the
    /// current state, and it is written only when it changed.
    pub fn change_control<T>(&self, change: impl FnOnce(&mut ControlState) -> T) -> Result<T> {
        self.store.change_control(change)
    }

    /// The file a session record lives in, by the month it started.
    #[must_use]
    pub fn path(&self, record: &SessionRecord) -> std::path::PathBuf {
        self.store.session_path(record)
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

    /// The config as `config.toml` (or the shipped defaults) holds it,
    /// without the store tags.
    ///
    /// # Errors
    ///
    /// The config's read errors.
    pub fn configured(&self) -> Result<Config> {
        self.store.config_only()
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

    /// Every configured tag and every tag that has entries, with counts.
    /// Reads only.
    pub fn list(&self) -> Result<TagList> {
        self.store.list_tags()
    }

    /// The definition `word` names, and whether it comes from `config` or
    /// the `store`. Reads only.
    pub fn definition(&self, word: &str) -> Result<(TagDef, &'static str)> {
        self.store.tag_definition(word)
    }

    /// The path of `config.toml`, whether or not it exists.
    #[must_use]
    pub fn config_path(&self) -> std::path::PathBuf {
        self.store.config_path()
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

    /// Run `f` over the group rows in one write step, `action` naming the
    /// write in a database error (`"join group in"`). Inside a [`Tx`] it is
    /// part of that write.
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

    /// One memory write in the write under way: `f` reads the rows and the
    /// files as the open journals leave them and returns what to commit,
    /// which is journaled, and applied to the files once the write commits.
    ///
    /// # Errors
    ///
    /// `f`'s error, and the store's write errors.
    pub fn commit<T>(
        &self,
        now: OffsetDateTime,
        f: impl FnOnce(&MemoryRows<'_>, &MemoryFiles) -> Result<(MemoryCommit, T)>,
    ) -> Result<(T, MemoryJournal)> {
        self.store.memory_commit(now, f)
    }

    /// Discard every earlier copy of the files `journal` removed, from open
    /// and closed journals, and truncate the log after the commit.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn scrub_copies(&self, journal: &MemoryJournal) -> Result<()> {
        self.store.scrub_copies(journal)
    }

    /// Profile revision `n`. Reads only.
    pub fn profile_revision(&self, n: i64) -> Result<ProfileRevision> {
        self.store.profile_revision(n)
    }

    /// The `now` items and `session`'s pending candidates the session pass
    /// reads before its model call. Reads only.
    pub fn context(
        &self,
        session: &SessionId,
    ) -> Result<(Vec<MemoryContext>, Vec<PendingContext>)> {
        self.store.memory_context(session)
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

    /// Active action entries past the stuck age at `now` with no pickup,
    /// counted per tag. Reads only.
    pub fn stuck_actions(&self, now: OffsetDateTime) -> Result<BTreeMap<TagName, usize>> {
        self.store.stuck_actions(now)
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

    /// Record one audit line. Inside a write's transaction it commits with
    /// the write. Otherwise it is inserted without waiting, and spooled to
    /// a file of its own when another writer holds the database or the
    /// store is still files, so no line waits on a lock or is lost to one.
    ///
    /// A store too new for this build takes no line.
    ///
    /// # Errors
    ///
    /// An unsafe data directory, or a spool file that cannot be written.
    pub fn record(&self, event: &AuditEvent) -> Result<()> {
        if self.store.open_conn().is_some() {
            return self.store.with_conn(|conn, path| {
                docs::put_audit(conn, path, audit::AUDIT_FORMAT, event, None, |e| {
                    db::db_error("audit to", path, e)
                })
                .map(drop)
            });
        }
        self.store.prepare_dir(&self.store.root)?;
        let moved = match self.store.documents_moved() {
            Ok(moved) => moved,
            // A store a newer build migrated takes no write from this one,
            // its log included; the command already reported the refusal.
            Err(Error::NewerFormat { .. }) => return Ok(()),
            Err(other) => return Err(other),
        };
        if moved {
            match self.store.audit_without_waiting(event) {
                Ok(()) => return Ok(()),
                Err(Error::Locked { .. }) => {}
                Err(other) => return Err(other),
            }
        }
        self.store.spool_audit(event)
    }

    /// The audit log, `<root>/audit.jsonl`.
    #[must_use]
    pub fn path(&self) -> std::path::PathBuf {
        self.store.audit_path()
    }
}
