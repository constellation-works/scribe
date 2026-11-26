//! Sessions in `scribe.db` (docs/design/scribe/2_design.md §15.7, phase 6):
//! where they are read from, `scribe store migrate`, `scribe export`, and
//! a session's forget.
//!
//! - **Reads.** Every session read comes from the database. A file store
//!   whose sessions never moved in is refused: this build has no file
//!   decoder (phase 8).
//! - **Migrate.** `scribe store migrate` imports the file store's archives
//!   into the archive tables now (`db/import.rs`), as the first write does on
//!   its own; it is refused while a session is on, while a rebuild
//!   checkpoint file waits, or when there is nothing to import.
//! - **Locks.** The per-session locks held across model calls (Codex
//!   ingestion, rebuild) are lock files in `locks/`; they hold no data.

mod export;

use super::Sessions;
use super::ids::resolve;
use super::memory::{MemoryFiles, MemoryRows, MemoryStep};
use super::{Access, ReadWrite};
use crate::WriteRefusal;
use crate::db::MemoryCommit;
use crate::db::files;
use crate::db::sessions::{self as rows, SessionTx};
use crate::db::{self, codec};
use crate::error::refusal;
use crate::error::{Error, Result};
use crate::message::NewMessage;
use crate::{
    CONTROL_FILE, ControlState, DB_FILE, MemoryWritten, RECORD_EXT, Redactor, Related,
    SESSIONS_DIR, SessionId, SessionRecord, Skipped, Store,
};
use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::path::PathBuf;
use time::OffsetDateTime;

impl<A: Access> Sessions<'_, A> {
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
        self.store.root.join(crate::DB_FILE)
    }

    /// The rebuild checkpoint of `original`'s session, if any.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn checkpoint(&self, original: &SessionRecord) -> Result<Option<SessionRecord>> {
        self.store.checkpoint_of(original)
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

    /// The ids of the forgotten sessions.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn forgotten_ids(&self) -> Result<Vec<SessionId>> {
        Ok(crate::db::sessions::read_forgotten(&self.store.root)?
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

    /// One session's record, its switch and the other records that read ([`Store::session`]).
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn get(&self, id: &SessionId) -> Result<SessionRow> {
        self.store.session(id)
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

    /// Whether `#scribe forget` erased `id`. Reads only.
    pub fn is_forgotten(&self, id: &SessionId) -> Result<bool> {
        crate::db::sessions::read_is_forgotten(&self.store.root, id)
    }

    /// The file a session record lives in, by the month it started.
    #[must_use]
    pub fn path(&self, record: &SessionRecord) -> std::path::PathBuf {
        self.store.session_path(record)
    }
}

/// The writes, on the repository a [`Tx`](super::Tx) gives: a
/// [`Snapshot`](super::Snapshot)'s cannot write.
impl Sessions<'_, ReadWrite> {
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
    pub fn put_from_checkpoint(&self, record: &SessionRecord) -> Result<()> {
        self.store.put_from_checkpoint(record)
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
    /// the memory `memory` plans (the caller's planner, run inside this
    /// transaction), and the switches as `change` leaves them.
    ///
    /// # Errors
    ///
    /// The planner's error, and the store's write errors.
    pub fn commit_window<F>(
        &self,
        record: Option<&SessionRecord>,
        messages: &[NewMessage],
        memory: MemoryStep<F>,
        redactor: &Redactor,
        change: impl FnOnce(&mut ControlState),
    ) -> Result<MemoryWritten>
    where
        F: FnOnce(&MemoryRows<'_>, &MemoryFiles) -> Result<(MemoryCommit, MemoryWritten)>,
    {
        self.store
            .commit_window(record, messages, memory, redactor, change)
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
        documents: impl FnOnce() -> Result<bool>,
    ) -> Result<SessionErasure> {
        self.store.erase_session(id, number, now, documents)
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

    /// Change the on/off state in the write under way: `change` gets the
    /// current state, and it is written only when it changed.
    pub fn change_control<T>(&self, change: impl FnOnce(&mut ControlState) -> T) -> Result<T> {
        self.store.change_control(change)
    }
}

/// One session record with its switch, and the other records it is read
/// beside.
#[derive(Debug)]
pub struct SessionRow {
    /// The record.
    pub record: SessionRecord,
    /// Its switch in `control.json`, if it has one.
    pub control: Option<crate::session::SessionControl>,
    /// Its `s-<n>`.
    pub number: Option<NonZeroU64>,
    /// When `#scribe forget` erased it, if it did.
    pub forgotten: Option<time::OffsetDateTime>,
    /// Every other record that read, in no particular order.
    pub others: Vec<SessionRecord>,
}

impl Store {
    /// The session record with this id, its switch, and the other records
    /// that read. A forgotten session's record is its tombstone.
    pub(crate) fn session(&self, id: &SessionId) -> Result<SessionRow> {
        let mut loaded = self.load_sessions()?;
        let record = match loaded.forgotten.get(id).copied() {
            Some(at) => forgotten_record(id, at),
            None => match loaded.take(id) {
                Some(record) => record?,
                None => {
                    return Err(Error::SessionNotFound {
                        id: id.clone(),
                        path: self.sessions_place()?,
                    });
                }
            },
        };
        let control = loaded.control;
        let number = loaded.numbers.get(id).copied();
        let forgotten = loaded.forgotten.get(id).copied();
        Ok(SessionRow {
            record,
            control: control.get(id).cloned(),
            number,
            forgotten,
            others: loaded.records,
        })
    }

    /// Where the sessions are, for a not-found error: `scribe.db` once they
    /// moved there, else `sessions/`.
    pub(crate) fn sessions_place(&self) -> Result<PathBuf> {
        Ok(
            if self.root_exists()? && db::sessions::read_moved(&self.root)? {
                self.root.join(DB_FILE)
            } else {
                self.root.join(SESSIONS_DIR)
            },
        )
    }

    /// The on/off state and Codex cursors, from `scribe.db`; empty before
    /// the first write. A file store is refused.
    pub(crate) fn control(&self) -> Result<ControlState> {
        if !self.root_exists()? {
            return Ok(ControlState::default());
        }
        match db::sessions::read_control(&self.root)? {
            Some(state) => self.check_stamp().map(|()| state),
            None => {
                self.refuse_file_store()?;
                Ok(ControlState::default())
            }
        }
    }

    /// Change the on/off state under the store lock: `change` gets the
    /// current state and its result is returned. `control.json` is
    /// rewritten only when the state changed. The data directory is created
    /// owner-only when absent, so a caller that may change nothing (the
    /// `Stop` hook) reads [`Store::control`] first and calls this only when
    /// a change is likely.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn update_control<T>(
        &self,
        change: impl FnOnce(&mut ControlState) -> T,
    ) -> Result<T> {
        self.prepare_dir(&self.root)?;
        self.write_step(|| self.change_control(change))
    }

    /// [`Store::update_control`] for a caller that holds the lock.
    /// Nothing is written when nothing changed: the lock keeps the state
    /// read here current.
    pub(crate) fn change_control<T>(
        &self,
        change: impl FnOnce(&mut ControlState) -> T,
    ) -> Result<T> {
        let mut state = self.control()?;
        let before = state.clone();
        let out = change(&mut state);
        if state != before {
            self.sessions_tx(|tx| tx.put_control(&before, &state))?;
        }
        Ok(out)
    }

    /// The file a session record lives in,
    /// `<root>/sessions/<yyyy>/<mm>/<id>.md`, by the month it started.
    #[must_use]
    pub(crate) fn session_path(&self, record: &SessionRecord) -> PathBuf {
        self.root
            .join(SESSIONS_DIR)
            .join(format!("{:04}", record.started.year()))
            .join(format!("{:02}", u8::from(record.started.month())))
            .join(format!("{}{RECORD_EXT}", record.id))
    }

    /// Commit one pass window in one `scribe.db` transaction: the record (its
    /// cursor advanced), the window's `messages`, the memory rows and
    /// journal `memory` plans, and the switches as `change` leaves them;
    /// the memory files follow the journal (2_design.md §15.6, §15.7).
    pub(crate) fn commit_window<F>(
        &self,
        record: Option<&SessionRecord>,
        messages: &[crate::message::NewMessage],
        memory: MemoryStep<F>,
        redactor: &Redactor,
        change: impl FnOnce(&mut ControlState),
    ) -> Result<MemoryWritten>
    where
        F: FnOnce(&MemoryRows<'_>, &MemoryFiles) -> Result<(MemoryCommit, MemoryWritten)>,
    {
        let conn = db::open_writer(&self.root, self.lock_wait)?;
        // Memory is planned first (any open journal replayed, `m-<n>` items
        // migrated); its rows and journal commit with the record below.
        let (memory_commit, memory_written) = match memory {
            MemoryStep::Skip => (db::MemoryCommit::default(), MemoryWritten::default()),
            step @ (MemoryStep::Replay | MemoryStep::Plan(_)) => {
                let mut writer = db::Writer::new(&conn, &self.root);
                self.window_memory(&mut writer, step)?
            }
        };
        let tx = db::sessions::SessionTx::begin(&conn, &self.root)?;
        let mut state = tx.control()?;
        let before = state.clone();
        if let Some(record) = record {
            tx.put_record(record)?;
            tx.upsert_messages(messages, redactor)?;
            // A pass forgets nothing, so it erases no message. The files
            // follow the journal after the commit.
            if !memory_commit.is_empty()
                && tx.stage_memory(&memory_commit, record.updated)?.0.is_some()
            {
                self.defer_journals();
            }
        }
        change(&mut state);
        if state != before {
            tx.put_control(&before, &state)?;
        }
        tx.commit()?;
        Ok(memory_written)
    }

    /// The session `input` names: `s-<n>`, a session with that exact
    /// provider id (a record or a switch), else the one session whose id
    /// starts with it (at least four characters). Reads only.
    ///
    /// # Errors
    ///
    /// [`Error::SessionNotFound`] when nothing matches; a usage error for a
    /// short input; an `id_ambiguous`
    /// [`Error::AgentWrite`] naming the candidates when several do; a store
    /// error otherwise.
    pub(crate) fn resolve_session(&self, input: &SessionId) -> Result<SessionId> {
        self.resolve_session_with(input, false, &[])
    }

    /// [`Store::resolve_session`], also accepting sessions known only through
    /// an active `handoff` entry (a session that was never switched on and
    /// has no session record; 2_design.md §16). Reads only.
    pub(crate) fn resolve_session_or_handoff(&self, input: &SessionId) -> Result<SessionId> {
        self.resolve_session_with(input, true, &[])
    }

    /// Every session id the store knows and `resolve_session_or_handoff`
    /// accepts: records (readable or not), switches, Codex cursors and
    /// sessions named only by an active handoff, and group members. The
    /// universe display prefixes must be unique in. Reads only.
    pub(crate) fn session_ids(&self) -> Result<Vec<String>> {
        let loaded = self.load_sessions()?;
        let handoffs: Vec<SessionId> = self
            .load_entries()?
            .0
            .iter()
            .filter(|e| {
                e.status == crate::EntryStatus::Active
                    && e.tag.as_str() == crate::handoff::HANDOFF_TAG
            })
            .flat_map(crate::entry::Entry::session_ids)
            .collect();
        let mut ids = Self::session_id_universe(&loaded, &handoffs);
        ids.extend(self.group_session_ids()?);
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    /// `related` with a `session:` target written as a unique id prefix
    /// replaced by the full id, so what is stored or compared is always the
    /// full id. A target that matches no known session stays as typed: the
    /// session may be one the store has never cataloged, and the command
    /// that reads the reference reports it. Any other kind is returned as
    /// given. Reads only.
    ///
    /// # Errors
    ///
    /// A usage error for a target shorter than four characters that is not a
    /// known id; `id_ambiguous` naming the candidates when several ids
    /// match.
    pub(crate) fn resolve_related(&self, related: &crate::Related) -> Result<crate::Related> {
        self.resolve_related_with(related, &[])
    }

    /// [`Store::resolve_related`], also counting `extra_ids` (such as
    /// uncataloged transcripts on disk) in the session id universe. Reads only.
    ///
    /// # Errors
    ///
    /// As [`Store::resolve_related`].
    pub(crate) fn resolve_related_with(
        &self,
        related: &crate::Related,
        extra_ids: &[String],
    ) -> Result<crate::Related> {
        if related.kind() != crate::RelatedKind::Session {
            return Ok(related.clone());
        }
        let Ok(input) = related.target().parse::<SessionId>() else {
            return Ok(related.clone());
        };
        match self.resolve_session_with(&input, true, extra_ids) {
            Ok(id) if id == input => Ok(related.clone()),
            Ok(id) => format!("session:{id}")
                .parse::<crate::Related>()
                .map_err(|e| refusal(WriteRefusal::InvalidId, e.to_string())),
            Err(Error::SessionNotFound { .. }) => Ok(related.clone()),
            Err(other) => Err(other),
        }
    }

    /// [`Store::resolve_session_or_handoff`], except that an id matching no
    /// known session is returned as typed: for a caller naming the session
    /// it runs in, which the store may never have cataloged.
    ///
    /// # Errors
    ///
    /// As [`Store::resolve_session_or_handoff`], except not-found.
    pub(crate) fn resolve_session_or_typed(&self, input: &SessionId) -> Result<SessionId> {
        self.resolve_session_or_typed_with(input, &[])
    }

    /// [`Store::resolve_session_or_typed`], also counting `extra_ids` (such as
    /// uncataloged transcripts on disk) in the session id universe. Reads only.
    ///
    /// # Errors
    ///
    /// As [`Store::resolve_session_or_typed`].
    pub(crate) fn resolve_session_or_typed_with(
        &self,
        input: &SessionId,
        extra_ids: &[String],
    ) -> Result<SessionId> {
        match self.resolve_session_with(input, true, extra_ids) {
            Err(Error::SessionNotFound { .. }) => Ok(input.clone()),
            other => other,
        }
    }

    /// [`Store::resolve_related`] over a list.
    ///
    /// # Errors
    ///
    /// The first error of [`Store::resolve_related`].
    pub(crate) fn resolve_relateds(
        &self,
        related: &[crate::Related],
    ) -> Result<Vec<crate::Related>> {
        self.resolve_relateds_with(related, &[])
    }

    /// [`Store::resolve_related_with`] over a list.
    ///
    /// # Errors
    ///
    /// The first error of [`Store::resolve_related_with`].
    pub(crate) fn resolve_relateds_with(
        &self,
        related: &[crate::Related],
        extra_ids: &[String],
    ) -> Result<Vec<crate::Related>> {
        related
            .iter()
            .map(|r| self.resolve_related_with(r, extra_ids))
            .collect()
    }

    fn group_session_ids(&self) -> Result<Vec<String>> {
        Ok(self
            .groups()?
            .into_iter()
            .flat_map(|g| {
                let mut ids: Vec<String> =
                    g.members().into_iter().map(|id| id.to_string()).collect();
                ids.push(g.opener);
                ids
            })
            .collect())
    }

    fn session_id_universe(
        loaded: &crate::repository::sessions::LoadedSessions,
        handoffs: &[SessionId],
    ) -> Vec<String> {
        let mut ids: Vec<String> = loaded
            .records
            .iter()
            .map(|r| r.id.as_str().to_owned())
            .chain(
                loaded
                    .skipped
                    .iter()
                    .filter_map(|(id, _)| id.as_ref().map(|id| id.as_str().to_owned())),
            )
            .chain(loaded.numbers.keys().map(|id| id.as_str().to_owned()))
            .chain(
                loaded
                    .control
                    .sessions
                    .keys()
                    .map(|id| id.as_str().to_owned()),
            )
            .chain(
                loaded
                    .control
                    .codex_cursors
                    .keys()
                    .map(|id| id.as_str().to_owned()),
            )
            .chain(handoffs.iter().map(|id| id.as_str().to_owned()))
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    fn resolve_session_with(
        &self,
        input: &SessionId,
        include_handoffs: bool,
        extra_ids: &[String],
    ) -> Result<SessionId> {
        let loaded = self.load_sessions()?;
        let handoff_sessions: Vec<SessionId> = if include_handoffs {
            self.load_entries()?
                .0
                .iter()
                .filter(|e| {
                    e.status == crate::EntryStatus::Active
                        && e.tag.as_str() == crate::handoff::HANDOFF_TAG
                })
                .flat_map(crate::entry::Entry::session_ids)
                .collect()
        } else {
            Vec::new()
        };
        let not_found = || -> Result<SessionId> {
            Err(Error::SessionNotFound {
                id: input.clone(),
                path: self.root.join(crate::SESSIONS_DIR),
            })
        };
        let mut ids = Self::session_id_universe(&loaded, &handoff_sessions);
        ids.extend(self.group_session_ids()?);
        ids.extend(extra_ids.iter().cloned());
        ids.sort();
        ids.dedup();
        // An exact provider id first; then `s-<n>`, the row id (2_design.md
        // §15.7), which a provider's UUID never looks like.
        if !ids.iter().any(|id| id == input.as_str())
            && let Some(n) = input
                .as_str()
                .strip_prefix("s-")
                .and_then(|n| n.parse::<u64>().ok())
        {
            return match loaded.numbers.iter().find(|(_, v)| v.get() == n) {
                Some((id, _)) => Ok(id.clone()),
                None => not_found(),
            };
        }
        match resolve(input.as_str(), &ids)? {
            Some(i) => ids[i]
                .parse::<SessionId>()
                .map_err(|e| refusal(WriteRefusal::InvalidId, e.to_string())),
            None => not_found(),
        }
    }
}

/// The directory of advisory lock files under the root.
pub const LOCKS_DIR: &str = "locks";

/// Every session scribe knows, from wherever they live
/// ([`crate::repository::Sessions::loaded`]).
#[derive(Debug, Default)]
pub struct LoadedSessions {
    /// The records that read, in no particular order.
    pub records: Vec<SessionRecord>,
    /// The records that did not, with their id when known.
    pub skipped: Vec<(Option<SessionId>, Skipped)>,
    /// Every session's `s-<n>`.
    pub numbers: BTreeMap<SessionId, NonZeroU64>,
    /// Every switch and Codex cursor.
    pub control: ControlState,
    /// When each forgotten session was forgotten.
    pub forgotten: BTreeMap<SessionId, OffsetDateTime>,
}

impl LoadedSessions {
    /// The record of `id`, or the error it failed to read with.
    pub fn take(&mut self, id: &SessionId) -> Option<Result<SessionRecord>> {
        if let Some(i) = self.records.iter().position(|r| &r.id == id) {
            return Some(Ok(self.records.swap_remove(i)));
        }
        let i = self
            .skipped
            .iter()
            .position(|(sid, _)| sid.as_ref() == Some(id))?;
        Some(Err(self.skipped.swap_remove(i).1.error))
    }
}

pub use crate::session::SessionForget;

/// What [`crate::repository::Sessions::erase`] erased of a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionErasure {
    /// Stored messages deleted.
    pub messages: usize,
    /// Segments deleted.
    pub segments: usize,
    /// Whether it was switched on, and is now off.
    pub was_on: bool,
    /// Whether it was forgotten before.
    pub already: bool,
    /// Whether anything changed.
    pub changed: bool,
    /// Archived records erased.
    pub files: usize,
}

impl Store {
    /// Every session, read-only, from `scribe.db`; none before the first
    /// write. A file store is refused.
    pub(crate) fn load_sessions(&self) -> Result<LoadedSessions> {
        if !self.root_exists()? {
            return Ok(LoadedSessions::default());
        }
        let Some(found) = rows::read_sessions(&self.root)? else {
            self.refuse_file_store()?;
            return Ok(LoadedSessions::default());
        };
        self.check_stamp()?;
        let mut loaded = LoadedSessions {
            numbers: found.numbers,
            control: found.control,
            forgotten: found.forgotten,
            ..LoadedSessions::default()
        };
        for row in found.records {
            match row {
                Ok((_, record)) if !loaded.forgotten.contains_key(&record.id) => {
                    loaded.records.push(record);
                }
                Ok(_) => {}
                Err(bad) => loaded.skipped.push((
                    bad.id,
                    Skipped {
                        path: bad.path,
                        error: bad.error,
                    },
                )),
            }
        }
        Ok(loaded)
    }

    /// Refuses a store whose `control.json` stamp is newer than this
    /// binary: the stamp is all that file holds. Read as plain JSON.
    pub(crate) fn check_stamp(&self) -> Result<()> {
        let path = self.root.join(CONTROL_FILE);
        if !self.root_exists()? || !cw_fs::inspect_file(&path)? {
            return Ok(());
        }
        codec::check_stamp(&path, &files::read(&path)?)
    }

    /// Every rebuild checkpoint file of a file store,
    /// `sessions/<yyyy>/<mm>/.<id>.rebuild.md`, by session.
    pub(crate) fn checkpoint_files(&self) -> Result<Vec<(SessionId, PathBuf)>> {
        let mut out = Vec::new();
        let dir = self.root.join(SESSIONS_DIR);
        if !cw_fs::inspect_dir(&dir)? {
            return Ok(out);
        }
        for year in cw_fs::list(&dir)? {
            let year_dir = dir.join(&year);
            if !cw_fs::inspect_dir(&year_dir)? {
                continue;
            }
            for month in cw_fs::list(&year_dir)? {
                let month_dir = year_dir.join(&month);
                if !cw_fs::inspect_dir(&month_dir)? {
                    continue;
                }
                for name in cw_fs::list(&month_dir)? {
                    let Some(id) = name
                        .strip_prefix('.')
                        .and_then(|n| n.strip_suffix(".rebuild.md"))
                    else {
                        continue;
                    };
                    let path = month_dir.join(&name);
                    if let (Ok(id), true) = (id.parse(), cw_fs::inspect_file(&path)?) {
                        out.push((id, path));
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// A lock file under `locks/`, the directory created owner-only when
    /// missing.
    pub(crate) fn lock_path(&self, name: &str) -> Result<PathBuf> {
        self.prepare_dir(&self.root)?;
        let dir = self.root.join(LOCKS_DIR);
        self.prepare_subdir(&dir)?;
        Ok(dir.join(name))
    }

    /// The rebuild checkpoint of `id`, read-only.
    pub(crate) fn checkpoint_of(&self, original: &SessionRecord) -> Result<Option<SessionRecord>> {
        let Some(text) = rows::read_checkpoint(&self.root, &original.id)? else {
            return Ok(None);
        };
        let path = rows::db_path(&self.root);
        codec::decode_session(&path, text.as_bytes(), original.id.as_str()).map(Some)
    }

    /// Run `f` in one session transaction on `scribe.db`, inside the store
    /// write it joins or one of its own (the first write moves any files
    /// in first). A write that refuses leaves everything where it was.
    pub(crate) fn sessions_tx<T>(&self, f: impl FnOnce(&SessionTx<'_>) -> Result<T>) -> Result<T> {
        self.write_step(|| {
            let conn = db::open_writer(&self.root, self.lock_wait)?;
            let tx = SessionTx::begin(&conn, &self.root)?;
            let out = f(&tx)?;
            tx.commit()?;
            Ok(out)
        })
    }

    /// Erase `id`'s rows, archives, documents and memory in the write under
    /// way, registering it first when it has no `s-<n>` (`number`).
    pub(crate) fn erase_session(
        &self,
        id: &SessionId,
        number: Option<NonZeroU64>,
        now: OffsetDateTime,
        documents: impl FnOnce() -> Result<bool>,
    ) -> Result<SessionErasure> {
        let (erased, already, changed, files) = self.with_conn(|conn, path| {
            if number.is_none() {
                rows::register(conn, path, id)?;
            }
            let (files, archive_changed) =
                crate::db::import::scrub_session(conn, path, id, number)?;
            let (erased, already, mut changed) =
                rows::erase(conn, path, id, now)?.ok_or_else(|| Error::SessionNotFound {
                    id: id.clone(),
                    path: path.to_path_buf(),
                })?;
            changed |= documents()?;
            changed |= self.erase_session_memory(id, now)?;
            changed |= archive_changed;
            if changed || files > 0 {
                db::truncate_wal_of(&self.root, self.lock_wait)?;
            }
            Ok((erased, already, changed, files))
        })?;
        Ok(SessionErasure {
            messages: erased.messages,
            segments: erased.segments,
            was_on: erased.was_on,
            already,
            changed,
            files,
        })
    }

    /// Whether a group seats `id`, in the write under way.
    pub(crate) fn has_group_membership(&self, id: &SessionId) -> Result<bool> {
        self.with_conn(|conn, path| rows::has_group_membership(conn, path, id))
    }

    /// Change the switches, and write a record, in one session transaction:
    /// `f` changes the state and returns the record to write, if any.
    pub(crate) fn change_sessions<T>(
        &self,
        f: impl FnOnce(&mut ControlState) -> Result<(T, Option<SessionRecord>)>,
    ) -> Result<T> {
        self.sessions_tx(|tx| {
            let mut state = tx.control()?;
            let before = state.clone();
            let (out, record) = f(&mut state)?;
            if state != before {
                tx.put_control(&before, &state)?;
            }
            if let Some(record) = record {
                tx.put_record(&record)?;
            }
            Ok(out)
        })
    }

    /// Write a rebuilt `record` and drop its checkpoint, together.
    pub(crate) fn put_from_checkpoint(&self, record: &SessionRecord) -> Result<()> {
        self.sessions_tx(|tx| {
            tx.put_record(record)?;
            tx.drop_checkpoint(&record.id)
        })
    }

    /// The `s-<n>` of every session, read-only.
    pub(crate) fn session_numbers(&self) -> Result<BTreeMap<SessionId, NonZeroU64>> {
        Ok(self.load_sessions()?.numbers)
    }
}

/// The existing session-show shape, populated solely from a tombstone.
/// It is never listed, exported as a record or offered as evidence.
pub(crate) fn forgotten_record(id: &SessionId, at: OffsetDateTime) -> SessionRecord {
    use crate::{Label, Project, SessionStatus};
    SessionRecord {
        id: id.clone(),
        status: SessionStatus::Final,
        active: false,
        keep_active: false,
        active_change: None,
        cataloged_by: None,
        rebuilt_by: None,
        project: Project::sanitized("deleted"),
        topics: Default::default(),
        related: Vec::new(),
        links: Vec::new(),
        terms: Vec::new(),
        provider: Label::sanitized("deleted", "deleted"),
        host: Label::sanitized("deleted", "deleted"),
        created: at,
        updated: at,
        title: String::new(),
        cwd: String::new(),
        started: at,
        on_since: at,
        scribed_through: None,
        passes: 0,
        next_open: 0,
        log: Vec::new(),
        summary: String::new(),
        goals: String::new(),
        next: String::new(),
    }
}
