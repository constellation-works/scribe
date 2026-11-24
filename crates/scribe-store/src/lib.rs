//! Persistence: the scribe store (`scribe.db` and the memory files) and
//! everything that touches disk.
//!
//! The business logic is in `scribe-domain`, with no I/O (`entry`, `tag`,
//! `session`, `proposal`, `config`, `verbatim`); this crate is the only one
//! that reads or writes the data root, so a second backend replaces this
//! crate and nothing else. `scribe-core` re-exports it as its `store`
//! module. It is split by responsibility:
//!
//! - this file, the [`Store`] facade: walk the layout, load, select;
//! - `codec`: the current format's documents, as the export, the memory
//!   files and rebuild checkpoints hold them, and the row checks;
//! - `archive`: the file store's archives in `scribe.db`, imported once and
//!   erased from by `forget` (phase 8);
//! - `audit`: the audit log's record shape;
//! - `db`: `scribe.db`, the SQLite database of every record kind
//!   (2_design.md §15.7), the only user of `rusqlite`;
//! - `write`: a store write as one `scribe.db` transaction, the archive
//!   import, and audit lines that never wait;
//! - `documents`: entries, claims, tags, marks, pickups and the audit log,
//!   read from the database;
//! - `fsio`: durable writes, locking and private-state checks, mechanism
//!   only.
//!
//! The layout (docs/design/scribe/2_design.md §3, §15.7):
//!
//! ```text
//! <root>/config.toml                        read-only to scribe
//! <root>/scribe.db                          every record kind (SQLite, WAL)
//! <root>/memory/                            profile.md, now/, milestones/
//! <root>/audit-spool/<time>-<pid>-<n>.json  audit lines a busy database could not take yet
//! <root>/control.json, proposals.json,      empty format stamps, so an older
//!   tags.json, marks.json, pickups.json       build refuses the store
//! <root>/locks/                             advisory locks (Codex catalog, rebuild)
//! ```
//!
//! The file store's `pre-sqlite-<date>/` and `pre-issue-<date>/` archives
//! are imported into `scribe.db` at the first write and then removed; a
//! file store whose records never moved in is refused, since this build has
//! no file decoder (2_design.md §15.7, phase 8).
//!
//! Reads take no lock and write nothing (STD-01 R31): SQLite's WAL gives a
//! reader a committed snapshot, and the CLI works on a read-only root. Every
//! load refuses state that is a link, another user's, or open to any group or
//! other mode bit (STD-05 R7–R9), and a newer format (STD-03 R10). A record
//! that does not read is refused naming it; a listing skips it and reports it
//! in [`Skipped`], so one bad record does not hide the rest (4_decisions.md,
//! "One malformed record makes its file corrupt").

#![deny(clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

mod archive;
mod audit;
mod candidates;
mod codec;
mod db;
pub use db::{ClaudeCapturePosition, ClaudeGroupCapture, DB_FILE};
mod documents;
pub use documents::Imported;
pub mod fsio;
mod ids;
mod memory;
pub use memory::{
    MEMORY_DIR, MILESTONES_DIR, MemoryShown, MemoryView, MemoryWritten, NOW_DIR, PROFILE_FILE,
    PassMemory, ProfileRevision,
};
pub use memory::{bare, not_found};
mod marks;
mod pickups;
mod rebuild;
pub mod repo;
pub use repo::{Snapshot, Tx};
#[cfg(feature = "test-support")]
mod seed;
mod sessions;
mod tags;
mod write;
pub use rebuild::SessionLock;
pub use sessions::{
    ExportCounts, ExportFormat, LOCKS_DIR, LoadedSessions, SessionErasure, SessionForget,
};

pub use audit::{AUDIT_FILE, AuditEvent, AuditStatus, IssueAudit};
pub use ids::resolve_prefix;
pub use redact::{MIN_SECRET_CHARS, REDACTED_FIELDS, Redactor, unresolved_redact_env};

// The domain at the paths this code had as scribe-core's `store` module.
#[cfg(test)]
use crate as store;
use crate::config::Config;
use crate::entry::{Entry, EntryFilter, EntryId};
use crate::error::{Error, Result};
use crate::proposal::Proposal;
use crate::record::Listing;
use crate::session::{
    self as session_mod, Child, ControlState, SessionFilter, SessionId, SessionRecord,
};
use scribe_domain::tag::{self, TagSummary};
use scribe_domain::{
    Author, AuthorKind, ClosedReason, ContentFrom, EntryStatus, Instruction, Label, LinkType,
    Project, ProposalId, ProposalKind, ProposalState, Related, RelatedKind, SessionError,
    SessionStatus, Speaker, StoreTag, TagDef, TagName, TagTable, WriteRefusal,
};
use scribe_domain::{
    action, annotation, brief, capture, checks, config, consistency, entry, error, group, handoff,
    instruction, message, models, proposal, record, redact, session,
};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The configuration file's name under the data directory.
pub const CONFIG_FILE: &str = "config.toml";
/// The entries directory's name under the data directory.
pub const ENTRIES_DIR: &str = "entries";
/// The session records directory's name under the data directory.
pub const SESSIONS_DIR: &str = "sessions";
/// The control document's name under the data directory.
pub const CONTROL_FILE: &str = "control.json";
/// The proposals document's name under the data directory.
pub const PROPOSALS_FILE: &str = "proposals.json";
/// Consumers' pickups of action entries, from format 8.
pub const PICKUPS_FILE: &str = "pickups.json";
/// Each `#scribe` annotation marker's outcome, from format 22
/// (2_design.md §14.5).
pub const MARKS_FILE: &str = "marks.json";

/// The legacy issue records' directory: a file store kind removed in
/// phase 2, refused like the rest of a file store.
const ISSUES_DIR: &str = "issues";

/// A record file's extension.
const RECORD_EXT: &str = ".md";

/// How long a writer waits for another writer before refusing (STD-03 R7):
/// `scribe.db`'s `busy_timeout`, and the per-session locks' wait.
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(10);

/// A record file a listing left out because it did not parse.
#[derive(Debug)]
pub struct Skipped {
    /// The file.
    pub path: PathBuf,
    /// Why: always [`Error::Corrupt`], which names the file.
    pub error: Error,
}

/// What [`Store::list_entries`] found.
#[derive(Debug)]
pub struct EntryList {
    /// The matching entries, oldest first.
    pub listing: Listing<Entry>,
    /// Entry files left out because they did not parse.
    pub skipped: Vec<Skipped>,
}

/// What [`Store::list_sessions`] found.
#[derive(Debug)]
pub struct SessionList {
    /// The matching session records, oldest first.
    pub listing: Listing<SessionRecord>,
    /// Every session's children, derived from all records' links (not only
    /// the listed ones), by the id they link to.
    pub children: BTreeMap<SessionId, Vec<Child>>,
    /// Every record's title, by id, for naming a linked session.
    pub titles: BTreeMap<SessionId, String>,
    /// Every session's on/off switch, to show beside its record.
    pub control: ControlState,
    /// Inactive sessions the filter left out that match its other
    /// criteria; `0` when it includes inactive ones.
    pub inactive_hidden: usize,
    /// Record files or rows left out because they did not parse.
    pub skipped: Vec<Skipped>,
    /// The session ids of the skipped records, where that much reads.
    pub skipped_sessions: Vec<SessionId>,
    /// Every session's `s-<n>`.
    pub numbers: BTreeMap<SessionId, NonZeroU64>,
    /// When each forgotten session was forgotten.
    pub forgotten: BTreeMap<SessionId, time::OffsetDateTime>,
}

/// One session record with its switch.
#[derive(Debug)]
pub struct SessionView {
    /// The record.
    pub record: SessionRecord,
    /// Its switch in `control.json`, if it has one.
    pub control: Option<crate::session::SessionControl>,
    /// The sessions that link to it, derived from their records.
    pub children: Vec<Child>,
    /// The titles of the sessions its links point at that have a record.
    pub linked_titles: BTreeMap<SessionId, String>,
    /// Its `s-<n>`.
    pub number: Option<NonZeroU64>,
    /// When `#scribe forget` erased it, if it did.
    pub forgotten: Option<time::OffsetDateTime>,
}

/// What [`Store::list_tags`] found.
#[derive(Debug)]
pub struct TagList {
    /// Every configured tag and every tag with entries, by name.
    pub tags: Vec<TagSummary>,
    /// Config overrides reported to the caller.
    pub warnings: Vec<String>,
    /// Entry files left out of the counts because they did not parse.
    pub skipped: Vec<Skipped>,
}

/// A scribe store rooted at one data directory.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
    lock_wait: Duration,
}

impl Store {
    /// A store at `root`. Performs no I/O. The caller resolves `root`
    /// (STD-02 R3): this crate never reads the environment or the home
    /// directory.
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            lock_wait: DEFAULT_LOCK_WAIT,
        }
    }

    /// Override how long a writer waits for the lock.
    #[must_use]
    pub fn with_lock_wait(mut self, wait: Duration) -> Self {
        self.lock_wait = wait;
        self
    }

    /// The data directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The audit log, `<root>/audit.jsonl`.
    #[must_use]
    pub(crate) fn audit_path(&self) -> PathBuf {
        self.root.join(AUDIT_FILE)
    }

    /// The file entry `id` lives in, `<root>/entries/<tag>/<id>.md`.
    #[must_use]
    pub(crate) fn entry_path(&self, id: &EntryId) -> PathBuf {
        self.root
            .join(ENTRIES_DIR)
            .join(id.tag().as_str())
            .join(format!("{id}{RECORD_EXT}"))
    }

    /// The configuration: `config.toml` parsed, or the defaults when there
    /// is none. A bad key or value is refused, naming it.
    pub(crate) fn config(&self) -> Result<Config> {
        let mut config = self.config_only()?;
        config.tags = self.merge_tags(&config.tags)?.0;
        Ok(config)
    }

    pub(crate) fn config_only(&self) -> Result<Config> {
        let path = self.root.join(CONFIG_FILE);
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(Config::default());
        }
        let bytes = fsio::read(&path)?;
        let text = std::str::from_utf8(&bytes).map_err(|e| Error::corrupt(&path, e))?;
        let config = Config::parse(text).map_err(|source| Error::Config { path, source })?;
        // Keys this build no longer reads: logged once per process, however
        // often a command loads the config.
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !config.warnings.is_empty() && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            for warning in &config.warnings {
                tracing::warn!("{warning}");
            }
        }
        Ok(config)
    }

    /// Active `handoff` entries referencing `session:<id>`, newest first.
    /// Reads only: no lock, no write.
    pub(crate) fn handoffs_for_session(&self, id: &SessionId) -> Result<Vec<Entry>> {
        let (entries, _) = self.load_entries()?;
        Ok(crate::handoff::for_session(&entries, id)
            .into_iter()
            .cloned()
            .collect())
    }

    /// Entries matching `filter`, oldest first, and how many matched. A
    /// missing store is an empty one.
    pub(crate) fn list_entries(&self, filter: &EntryFilter) -> Result<EntryList> {
        self.list_entries_scoped(filter, None)
    }

    /// List entries, optionally restricted to the group's member sessions
    /// before applying the filter and its limit.
    pub(crate) fn list_entries_scoped(
        &self,
        filter: &EntryFilter,
        members: Option<&BTreeSet<SessionId>>,
    ) -> Result<EntryList> {
        let (mut entries, skipped) = self.load_entries()?;
        if let Some(members) = members {
            entries.retain(|entry| crate::group::entry_belongs(entry, members));
        }
        Ok(EntryList {
            listing: filter.select(entries),
            skipped,
        })
    }

    /// Session records matching `filter`, oldest first, with the on/off
    /// state beside them.
    pub(crate) fn list_sessions(&self, filter: &SessionFilter) -> Result<SessionList> {
        self.list_sessions_scoped(filter, None)
    }

    /// List session records, optionally restricting the records and their
    /// derived lookup data to the group's member sessions.
    pub(crate) fn list_sessions_scoped(
        &self,
        filter: &SessionFilter,
        members: Option<&BTreeSet<SessionId>>,
    ) -> Result<SessionList> {
        let mut loaded = self.load_sessions()?;
        if let Some(members) = members {
            loaded.records.retain(|r| members.contains(&r.id));
            loaded
                .skipped
                .retain(|(id, _)| id.as_ref().is_some_and(|id| members.contains(id)));
        }
        let control = loaded.control;
        let records = loaded.records;
        let skipped_sessions = loaded
            .skipped
            .iter()
            .filter_map(|(id, _)| id.clone())
            .collect();
        let skipped: Vec<Skipped> = loaded.skipped.into_iter().map(|(_, s)| s).collect();
        let numbers = loaded.numbers;
        let forgotten = loaded.forgotten;
        let inactive_hidden = if filter.include_inactive {
            0
        } else {
            let any = SessionFilter {
                include_inactive: true,
                ..filter.clone()
            };
            records
                .iter()
                .filter(|r| !r.active && any.matches(r))
                .count()
        };
        Ok(SessionList {
            children: session_mod::children(&records),
            titles: records
                .iter()
                .map(|r| (r.id.clone(), r.title.clone()))
                .collect(),
            listing: filter.select(records),
            inactive_hidden,
            control,
            skipped,
            skipped_sessions,
            numbers,
            forgotten,
        })
    }

    /// The session record with this id, its switch, the sessions that link
    /// to it and the titles of those it links to. Other records that do not
    /// parse are left out of the children.
    pub(crate) fn session(&self, id: &SessionId) -> Result<SessionView> {
        let mut loaded = self.load_sessions()?;
        let record = match loaded.forgotten.get(id).copied() {
            Some(at) => sessions::forgotten_record(id, at),
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
        let mut others = loaded.records;
        let linked_titles = others
            .iter()
            .filter(|r| record.links.iter().any(|l| l.session == r.id))
            .map(|r| (r.id.clone(), r.title.clone()))
            .collect();
        others.push(record.clone());
        let children = session_mod::children(&others)
            .remove(id)
            .unwrap_or_default();
        Ok(SessionView {
            record,
            control: control.get(id).cloned(),
            children,
            linked_titles,
            number,
            forgotten,
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

    /// Every configured tag and every tag that has entries, with counts.
    pub(crate) fn list_tags(&self) -> Result<TagList> {
        let config = self.config_only()?;
        let stored = self.store_tags()?;
        let (table, mut warnings) = config
            .tags
            .merge_store(&stored)
            .map_err(|e| crate::checks::refusal(crate::WriteRefusal::TagConflict, e.to_string()))?;
        warnings.extend(config.tag_warnings);
        let configured = &config.tags;
        let (entries, skipped) = self.load_entries()?;
        let pickups = self.pickups()?;
        let counts = crate::action::tag_counts(&entries, &table, &pickups);
        let mut tags = tag::summarize(&table, &entries);
        for tag in &mut tags {
            if let Some((retired, rejected)) = counts.get(&tag.name) {
                tag.retired = *retired;
                tag.rejected = *rejected;
            }
            tag.source = if configured.get(&tag.name).is_some() {
                Some("config")
            } else if table.get(&tag.name).is_some() {
                Some("store")
            } else {
                None
            };
            if tag.source == Some("store") {
                tag.author = stored
                    .iter()
                    .find(|t| t.name == tag.name)
                    .map(|t| t.author.clone());
            }
        }
        Ok(TagList {
            tags,
            skipped,
            warnings,
        })
    }

    /// The entries already recorded from one prompt: `session` and
    /// `prompt_sha256` as their cite has them. Reads only, so `Stop` can
    /// skip a `#scribe` line an entry was already written for.
    pub(crate) fn entries_from_prompt(
        &self,
        session: &SessionId,
        prompt_sha256: &str,
    ) -> Result<Vec<Entry>> {
        Ok(self
            .load_entries()?
            .0
            .into_iter()
            .filter(|e| {
                e.cite
                    .as_ref()
                    .is_some_and(|c| &c.session == session && c.prompt_sha256 == prompt_sha256)
            })
            .collect())
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
    fn change_control<T>(&self, change: impl FnOnce(&mut ControlState) -> T) -> Result<T> {
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
    /// journal planned from `memory`, and the switches as `change` leaves
    /// them; the memory files follow the journal (2_design.md §15.6, §15.7).
    pub(crate) fn commit_window(
        &self,
        record: Option<&SessionRecord>,
        messages: &[crate::message::NewMessage],
        memory: Option<&PassMemory>,
        sensitive: &[scribe_domain::memory::Refusal],
        redactor: &Redactor,
        change: impl FnOnce(&mut ControlState),
    ) -> Result<MemoryWritten> {
        let conn = db::open_writer(&self.root, self.lock_wait)?;
        // Memory is planned first (any open journal replayed, `m-<n>` items
        // migrated); its rows and journal commit with the record below.
        let (memory_commit, memory_written) = match (memory, record) {
            (Some(memory), Some(record)) => {
                let mut writer = db::Writer::new(&conn, &self.root);
                self.plan_pass_memory(&mut writer, memory, sensitive, record)?
            }
            _ => (db::MemoryCommit::default(), MemoryWritten::default()),
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

    /// Keep a pass window's messages in `scribe.db` (2_design.md §15.7),
    /// each text redacted first, upserted on (session, message id) in one
    /// `BEGIN IMMEDIATE` transaction. The pass calls this before it writes
    /// the session file, whose cursor stays the truth: a window written
    /// again rewrites the same rows. Returns how many rows changed.
    ///
    /// # Errors
    ///
    /// [`Error::Locked`] when another writer holds the database past the
    /// lock wait; the privacy errors when the root or the database is a
    /// link, another user's or open to others; [`Error::NewerFormat`] for a
    /// database a newer build wrote.
    pub(crate) fn write_messages(
        &self,
        messages: &[crate::message::NewMessage],
        redactor: &Redactor,
    ) -> Result<usize> {
        if messages.is_empty() {
            return Ok(0);
        }
        self.prepare_dir(&self.root)?;
        db::upsert_messages(&self.root, self.lock_wait, messages, redactor)
    }

    /// Delete stored messages by (session, message id), for a forget,
    /// leaving no trace of their text in the database or its WAL. A missing
    /// database deletes nothing.
    fn forget_messages(&self, keys: &[(SessionId, String)]) -> Result<usize> {
        db::delete_messages(&self.root, self.lock_wait, keys)
    }

    /// Delete the stored messages a forgotten `entry` rests on: the turn it
    /// cites and the turns its candidate proposals cite.
    fn forget_cited_messages(&self, entry: &Entry, proposals: &[Proposal]) -> Result<usize> {
        let keys: Vec<_> = entry
            .cite
            .iter()
            .chain(
                proposals
                    .iter()
                    .filter(|p| p.older == entry.id)
                    .filter_map(|p| p.candidate.as_ref().map(|c| &c.cite)),
            )
            .filter_map(|c| c.message.clone().map(|m| (c.session.clone(), m)))
            .collect();
        self.forget_messages(&keys)
    }

    /// The stored messages matching any of `tokens` (ranking tokens, as
    /// [`scribe_domain::text::tokens`] gives them) as a word prefix, best FTS5
    /// `bm25()` first. Read-only: no lock, no write (STD-01 R31). A missing
    /// root or database matches nothing.
    ///
    /// # Errors
    ///
    /// The privacy errors for the root or the database, [`Error::NewerFormat`]
    /// and [`Error::Database`].
    pub(crate) fn search_messages(
        &self,
        tokens: &[String],
    ) -> Result<crate::message::MessageSearch> {
        if !self.root_exists()? {
            return Ok(crate::message::MessageSearch::default());
        }
        db::search_messages(&self.root, tokens, None)
    }

    fn prepare_dir(&self, dir: &Path) -> Result<()> {
        if !fsio::inspect_dir(dir)? {
            fsio::create_private_dir(dir)?;
        }
        Ok(())
    }

    fn prepare_subdir(&self, dir: &Path) -> Result<()> {
        if !fsio::inspect_subdir(dir)? {
            fsio::create_private_dir(dir)?;
        }
        Ok(())
    }

    fn root_exists(&self) -> Result<bool> {
        fsio::inspect_dir(&self.root)
    }
}

/// Names that can be records: not hidden. A writer stages its temporary file
/// as a dot-file beside the target, and an editor leaves swap files the
/// same way; neither is a record.
fn visible(names: Vec<String>) -> impl Iterator<Item = String> {
    names.into_iter().filter(|n| !n.starts_with('.'))
}

#[cfg(any(test, feature = "test-support"))]
pub mod testing;
#[cfg(test)]
mod tests;
