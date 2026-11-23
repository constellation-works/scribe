//! Sessions in `scribe.db` (docs/design/scribe/2_design.md §15.7, §15.11,
//! phase 6): where they are read from, how the files move in, and
//! `scribe export`.
//!
//! - **Reads.** Once `record_kinds` holds `sessions`, every session read
//!   comes from the database; before that, from the format-25 files, with
//!   today's decoders (kept for the 30-day window). A file store's `s-<n>`
//!   numbers are the ones the move will give: records by `created`, then
//!   id, and after them the sessions with only a switch, by id.
//! - **The move.** The first write's transaction moves the sessions in
//!   ([`Store::import_sessions`]), checking the record count, with the
//!   other record kinds (`documents.rs`); after the commit the files go to
//!   `pre-sqlite-<date>/`, which scribe never deletes, and `control.json`
//!   stays as a format stamp with no sessions, so an older build refuses
//!   the store instead of starting a second set of files. `scribe store
//!   migrate` is the same move run by hand, refused while a session is on,
//!   while a rebuild checkpoint waits, or once everything moved.
//! - **Locks.** The per-session locks held across model calls (Codex
//!   ingestion, rebuild) are lock files in `locks/`; they hold no data.

use super::db::sessions::{self as rows, SessionTx};
use super::write::Move;
use super::{
    CONTROL_FILE, Moved, RECORD_EXT, SESSIONS_DIR, Skipped, Store, agent::refusal, db, format, fsio,
};
use crate::WriteRefusal;
use crate::error::{Error, Result};
use crate::session::{ControlState, SessionId, SessionRecord};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// The directory of advisory lock files under the root.
pub const LOCKS_DIR: &str = "locks";

/// What the moved-aside files directory is called: `pre-sqlite-<date>`.
pub(super) const MOVED_PREFIX: &str = "pre-sqlite-";

/// Every session scribe knows, from wherever they live.
#[derive(Debug, Default)]
pub(super) struct Loaded {
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

impl Loaded {
    /// The record of `id`, or the error it failed to read with.
    pub(super) fn take(&mut self, id: &SessionId) -> Option<Result<SessionRecord>> {
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

/// What `#scribe forget s-<n>` did ([`Store::settle`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionForget {
    /// The provider's session id.
    pub id: SessionId,
    /// Its `s-<n>`.
    pub number: Option<NonZeroU64>,
    /// Stored messages deleted.
    pub messages: usize,
    /// Segments deleted.
    pub segments: usize,
    /// Archived record and checkpoint files scheduled for erasure.
    pub files: usize,
    /// Whether it was switched on, and is now off.
    pub was_on: bool,
}

/// Result of forgetting a session, including cleanup on a replay.
pub(super) enum SessionForgetOutcome {
    Forgotten(SessionForget),
    Already {
        id: SessionId,
        number: Option<NonZeroU64>,
        cleaned: bool,
    },
}

/// The format of `scribe export`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// One file per session, the shape of today's record files, and
    /// `control.json`.
    Markdown,
    /// One line per row, one file per table.
    Jsonl,
}

/// What `scribe export` wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Exported {
    /// The files written, relative to the target directory, in order.
    pub files: Vec<String>,
    /// Sessions written.
    pub sessions: usize,
    /// Entries written.
    pub entries: usize,
    /// Claims written.
    pub claims: usize,
    /// Audit lines written.
    pub audit_lines: usize,
    /// Records that did not read, so were not written.
    pub skipped: Vec<PathBuf>,
}

impl Store {
    /// Every session, read-only: from `scribe.db` once the sessions moved
    /// there, else from the files.
    pub(super) fn load_sessions(&self) -> Result<Loaded> {
        if !self.root_exists()? {
            return Ok(Loaded::default());
        }
        let Some(found) = rows::read_sessions(&self.root)? else {
            return self.load_session_files();
        };
        self.check_stamp()?;
        let mut loaded = Loaded {
            numbers: found.numbers,
            control: found.control,
            forgotten: found.forgotten,
            ..Loaded::default()
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

    /// Every session of a file store, numbered as the move will number
    /// them.
    fn load_session_files(&self) -> Result<Loaded> {
        let mut loaded = Loaded {
            control: self.control_file()?,
            ..Loaded::default()
        };
        for file in self.session_files()? {
            match file.load() {
                Ok(record) => loaded.records.push(record),
                Err(error @ Error::Corrupt { .. }) => loaded.skipped.push((
                    file.stem.parse().ok(),
                    Skipped {
                        path: file.path,
                        error,
                    },
                )),
                Err(other) => return Err(other),
            }
        }
        let mut order: Vec<&SessionRecord> = loaded.records.iter().collect();
        order.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
        let mut ids: Vec<SessionId> = order.into_iter().map(|r| r.id.clone()).collect();
        ids.extend(
            loaded
                .control
                .sessions
                .keys()
                .filter(|id| !loaded.records.iter().any(|r| &r.id == *id))
                .cloned(),
        );
        for (i, id) in ids.into_iter().enumerate() {
            if let Some(n) = NonZeroU64::new(u64::try_from(i + 1).unwrap_or(u64::MAX)) {
                loaded.numbers.insert(id, n);
            }
        }
        Ok(loaded)
    }

    /// `control.json` as a format-25 store holds it; empty when absent.
    pub(super) fn control_file(&self) -> Result<ControlState> {
        let path = self.root.join(CONTROL_FILE);
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(ControlState::default());
        }
        format::decode_control(&path, &fsio::read(&path)?)
    }

    /// Refuses a store whose `control.json` stamp is newer than this
    /// binary, after the sessions moved: the stamp is all that file holds.
    pub(super) fn check_stamp(&self) -> Result<()> {
        self.control_file().map(drop)
    }

    /// In the first write's transaction: move the sessions in when they
    /// are still files (§15.11): every record checked and imported in
    /// `created` order, then the switch-only sessions, the Codex cursors
    /// and (at a write) the rebuild checkpoints, checking the record count.
    /// A store that never had any is marked moved.
    pub(super) fn import_sessions(&self, mode: Move, now: OffsetDateTime) -> Result<Moved> {
        let conn = db::open_writer(&self.root, self.lock_wait)?;
        let tx = SessionTx::begin(&conn, &self.root)?;
        if tx.moved()? {
            tx.commit()?;
            return Ok(Moved::default());
        }
        let control = self.control_file()?;
        let checkpoints = if mode == Move::AtWrite {
            self.checkpoint_files()?
        } else {
            Vec::new()
        };
        let mut records = Vec::new();
        for file in self.session_files()? {
            // Every record must read (check_log included) before any is
            // imported: a broken file is fixed or removed by the user.
            records.push(file.load()?);
        }
        records.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
        for record in &records {
            tx.put_record(record)?;
        }
        tx.put_control(&ControlState::default(), &control)?;
        let mut imported_checkpoints = 0;
        for (id, path) in &checkpoints {
            let record = self.decode_checkpoint_file(id, path)?;
            let text = encode_checkpoint(path, &record)?;
            if tx.put_checkpoint(id, &text, now)? {
                imported_checkpoints += 1;
            }
        }
        if tx.record_count()? != records.len() {
            return Err(Error::corrupt(
                rows::db_path(&self.root),
                format!(
                    "the move found {} session records but scribe.db holds a different count",
                    records.len()
                ),
            ));
        }
        // The directory the session files move to, `None` when there are none
        // (`record_kinds.moved_to`).
        let moved_to =
            (!self.session_files_to_move_aside()?.is_empty()).then(|| moved_dir_name(now));
        tx.mark_moved(now, moved_to.as_deref())?;
        tx.commit()?;
        self.after(|a| a.aside = true);
        Ok(Moved {
            sessions: records.len(),
            switches: control.sessions.len(),
            cursors: control.codex_cursors.len(),
            checkpoints: imported_checkpoints,
            ..Moved::default()
        })
    }

    /// `scribe store migrate`: move every record kind still in files into
    /// `scribe.db` in one transaction.
    ///
    /// # Errors
    ///
    /// `migrate_blocked` while any session is switched on, while any rebuild
    /// checkpoint exists, or when everything already moved; `store_corrupt`
    /// naming a record that does not read; store failures.
    pub fn migrate_store(&self, now: OffsetDateTime) -> Result<Moved> {
        let mut moved = self.transaction_as(Move::ByHand, now, || {
            self.with_conn(|_, _| Ok(()))?;
            let mut moved = self.moved_now().unwrap_or_default();
            if self.will_move_aside()? {
                moved.moved_to = Some(moved_dir_name(now));
            }
            Ok::<_, Error>(moved)
        })?;
        let aside = moved_dir_name(now);
        if fsio::inspect_subdir(&self.root.join(&aside))? {
            moved.moved_to = Some(aside);
        }
        Ok(moved)
    }

    /// Every rebuild checkpoint file of a file store,
    /// `sessions/<yyyy>/<mm>/.<id>.rebuild.md`, by session.
    pub(super) fn checkpoint_files(&self) -> Result<Vec<(SessionId, PathBuf)>> {
        let mut out = Vec::new();
        let dir = self.root.join(SESSIONS_DIR);
        if !fsio::inspect_subdir(&dir)? {
            return Ok(out);
        }
        for year in fsio::list_dir(&dir)? {
            let year_dir = dir.join(&year);
            if !fsio::inspect_subdir(&year_dir)? {
                continue;
            }
            for month in fsio::list_dir(&year_dir)? {
                let month_dir = year_dir.join(&month);
                if !fsio::inspect_subdir(&month_dir)? {
                    continue;
                }
                for name in fsio::list_dir(&month_dir)? {
                    let Some(id) = name
                        .strip_prefix('.')
                        .and_then(|n| n.strip_suffix(".rebuild.md"))
                    else {
                        continue;
                    };
                    let path = month_dir.join(&name);
                    if let (Ok(id), true) = (id.parse(), fsio::inspect_file(&path)?) {
                        out.push((id, path));
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    fn decode_checkpoint_file(&self, id: &SessionId, path: &Path) -> Result<SessionRecord> {
        format::decode_session_at(path, &fsio::read(path)?, id.as_str(), None)
    }

    /// A lock file under `locks/`, the directory created owner-only when
    /// missing.
    pub(super) fn lock_path(&self, name: &str) -> Result<PathBuf> {
        self.prepare_dir(&self.root)?;
        let dir = self.root.join(LOCKS_DIR);
        self.prepare_subdir(&dir)?;
        Ok(dir.join(name))
    }

    /// The rebuild checkpoint of `id`, read-only.
    pub(super) fn checkpoint_of(&self, original: &SessionRecord) -> Result<Option<SessionRecord>> {
        if let Some(text) = rows::read_checkpoint(&self.root, &original.id)? {
            let path = rows::db_path(&self.root);
            return format::decode_session_at(&path, text.as_bytes(), original.id.as_str(), None)
                .map(Some);
        }
        if rows::read_moved(&self.root)? {
            return Ok(None);
        }
        let path = self
            .session_path(original)
            .with_file_name(format!(".{}.rebuild.md", original.id));
        if !fsio::inspect_file(&path)? {
            return Ok(None);
        }
        self.decode_checkpoint_file(&original.id, &path).map(Some)
    }

    /// Run `f` in one session transaction on `scribe.db`, inside the store
    /// write it joins or one of its own (the first write moves any files
    /// in first). A write that refuses leaves everything where it was.
    pub(super) fn sessions_tx<T>(&self, f: impl FnOnce(&SessionTx<'_>) -> Result<T>) -> Result<T> {
        self.write(|| {
            let conn = db::open_writer(&self.root, self.lock_wait)?;
            let tx = SessionTx::begin(&conn, &self.root)?;
            let out = f(&tx)?;
            tx.commit()?;
            Ok(out)
        })
    }

    /// `scribe export`: write every record to `target`, a directory that is
    /// missing or empty, created owner-only. Reads the store only: no lock,
    /// no store write (STD-01 R31).
    ///
    /// # Errors
    ///
    /// `export_target_exists` for a directory with anything in it;
    /// `export_needs_migrate` for JSONL of records still in files; I/O and
    /// store failures.
    pub fn export(&self, target: &Path, as_format: ExportFormat) -> Result<Exported> {
        let mut out = Exported::default();
        let rows = match as_format {
            ExportFormat::Markdown => None,
            ExportFormat::Jsonl if !self.root_exists()? => Some(Vec::new()),
            ExportFormat::Jsonl => {
                let db = db::read_export(&self.root, SESSION_EXPORT_TABLES, DOC_EXPORT_TABLES)?;
                let (spooled, _) = self.spooled_audit()?;
                if (!db.sessions_moved || !db.docs_moved)
                    && (self.has_files_to_move_aside()? || !spooled.is_empty())
                {
                    return Err(refusal(
                        WriteRefusal::ExportNeedsMigrate,
                        "JSONL export reads scribe.db, and this store's records are still files; run `scribe store migrate`, or export --as markdown",
                    ));
                }
                self.check_stamp()?;
                let mut rows = db.rows;
                if db.docs_moved && !spooled.is_empty() {
                    let drained: BTreeSet<&str> = rows
                        .iter()
                        .filter(|(table, _)| *table == "audit")
                        .filter_map(|(_, row)| row.get("spool").and_then(serde_json::Value::as_str))
                        .collect();
                    let spool_dir = self.root.join(super::write::AUDIT_SPOOL_DIR);
                    let mut pending = Vec::with_capacity(spooled.len());
                    for (name, format, event) in &spooled {
                        if !drained.contains(name.as_str()) {
                            pending.push((
                                "audit",
                                super::db::docs::audit_export_row(
                                    &spool_dir.join(name),
                                    *format,
                                    event,
                                    name,
                                )?,
                            ));
                        }
                    }
                    rows.extend(pending);
                }
                Some(rows)
            }
        };
        prepare_target(target)?;
        let mut files = Vec::new();
        let mut write = |rel: String, bytes: &[u8]| -> Result<()> {
            let path = target.join(&rel);
            let mut dir = target.to_path_buf();
            for part in Path::new(&rel)
                .parent()
                .into_iter()
                .flat_map(Path::components)
            {
                dir.push(part);
                if !fsio::inspect_subdir(&dir)? {
                    fsio::create_private_dir(&dir)?;
                }
            }
            fsio::write_atomic(&path, bytes)?;
            files.push(rel);
            Ok(())
        };
        match rows {
            Some(rows) => {
                let mut tables: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
                for (table, row) in rows {
                    let line = tables.entry(table).or_default();
                    serde_json::to_writer(&mut *line, &row).map_err(|e| {
                        Error::corrupt(target, format!("a row does not encode: {e}"))
                    })?;
                    line.push(b'\n');
                }
                let lines = |name: &str| {
                    tables
                        .get(name)
                        .map_or(0, |b| b.iter().filter(|c| **c == b'\n').count())
                };
                out.sessions = lines("sessions");
                out.entries = lines("entries");
                out.claims = lines("claims");
                out.audit_lines = lines("audit");
                for (table, bytes) in tables {
                    write(format!("{table}.jsonl"), &bytes)?;
                }
            }
            None => {
                let loaded = self.load_sessions()?;
                let mut records = loaded.records;
                records.sort_by(|a, b| a.started.cmp(&b.started).then_with(|| a.id.cmp(&b.id)));
                for record in &records {
                    let rel = format!(
                        "{SESSIONS_DIR}/{:04}/{:02}/{}{RECORD_EXT}",
                        record.started.year(),
                        u8::from(record.started.month()),
                        record.id
                    );
                    let bytes = format::encode_session(&target.join(&rel), record)?;
                    write(rel, &bytes)?;
                }
                let path = target.join(CONTROL_FILE);
                write(
                    CONTROL_FILE.to_owned(),
                    &format::encode_control(&path, &loaded.control)?,
                )?;
                out.sessions = records.len();
                out.skipped = loaded.skipped.into_iter().map(|(_, s)| s.path).collect();
                self.export_documents(target, &mut write, &mut out)?;
            }
        }
        out.files = files;
        Ok(out)
    }

    /// Erase session data and detach dependent records in the transaction
    /// the typed hook uses for its audit. The tombstone also drives replay
    /// of filesystem cleanup after commit.
    pub(super) fn forget_session_locked(
        &self,
        input: &SessionId,
        now: OffsetDateTime,
    ) -> Result<SessionForgetOutcome> {
        let id = match self.resolve_session_or_handoff(input) {
            Ok(id) => id,
            Err(error @ Error::SessionNotFound { .. }) => {
                let known = self
                    .with_conn(|conn, path| rows::has_group_membership(conn, path, input))?
                    || self
                        .load_entries()?
                        .0
                        .iter()
                        .any(|entry| crate::handoff::session_ids(entry).contains(input));
                if !known {
                    return Err(error);
                }
                input.clone()
            }
            Err(error) => return Err(error),
        };
        let number = self.session_numbers()?.get(&id).copied();
        let files = self.moved_files_of(&id)?.len() + self.legacy_files_to_forget(&id)?;
        let archive_changed = !self.moved_session_documents(&id, number, now)?.is_empty();
        let (erased, already, changed) = self.with_conn(|conn, path| {
            if number.is_none() {
                rows::register(conn, path, &id)?;
            }
            let (erased, already, mut changed) =
                rows::forget(conn, path, &id, now)?.ok_or_else(|| Error::SessionNotFound {
                    id: id.clone(),
                    path: path.to_path_buf(),
                })?;
            changed |= self.forget_session_documents(&id, now)?;
            changed |= self.forget_session_memory(&id, now)?;
            if changed || files > 0 {
                db::truncate_wal_of(&self.root, self.lock_wait)?;
            }
            Ok((erased, already, changed))
        })?;
        let number = self.session_numbers()?.get(&id).copied();
        Ok(if already {
            SessionForgetOutcome::Already {
                id,
                number,
                cleaned: changed || files > 0 || archive_changed,
            }
        } else {
            SessionForgetOutcome::Forgotten(SessionForget {
                id,
                number,
                messages: erased.messages,
                segments: erased.segments,
                files,
                was_on: erased.was_on,
            })
        })
    }

    /// A forgotten row is the durable obligation to remove legacy files.
    /// Replay after every commit, including a restarted migration, so an
    /// interruption between the database commit and the archive move cannot
    /// leave a forgotten session in plaintext.
    ///
    /// The archive lock is taken only when a moved archive still holds a
    /// file of a forgotten session, so a store with nothing to erase never
    /// fails an already-committed write on someone else's lock. The scan
    /// repeats under the lock before anything is deleted.
    pub(super) fn clean_forgotten_session_files(&self) -> Result<()> {
        let Some(forgotten) = rows::read_forgotten(&self.root)? else {
            return Ok(());
        };
        if forgotten.is_empty() {
            return Ok(());
        }
        self.check_stamp()?;
        let changes = |id: &SessionId,
                       tombstone: rows::Tombstone|
         -> Result<Vec<(PathBuf, Option<Vec<u8>>)>> {
            let mut changes =
                self.moved_session_documents(id, Some(tombstone.number), tombstone.at)?;
            changes.extend(
                self.moved_files_of(id)?
                    .into_iter()
                    .map(|path| (path, None)),
            );
            Ok(changes)
        };
        for (id, tombstone) in forgotten {
            if changes(&id, tombstone)?.is_empty() {
                continue;
            }
            let lock_path = self.lock_path(super::documents::ARCHIVE_FORGET_LOCK)?;
            let _lock =
                fsio::LockGuard::acquire(&lock_path, "forget moved session", self.lock_wait)?;
            let mut changes = changes(&id, tombstone)?;
            // Rewrite documents that reference archived handoff ids before
            // deleting the files that identify them.
            changes.sort_by_key(|(_, content)| content.is_none());
            for (path, content) in changes {
                match content {
                    Some(bytes) => fsio::write_atomic(&path, &bytes)?,
                    None => fsio::remove_private(&path)?,
                }
            }
        }
        Ok(())
    }

    /// Archived record and checkpoint files belonging to `id`.
    fn moved_files_of(&self, id: &SessionId) -> Result<Vec<PathBuf>> {
        let mut found = Vec::new();
        for moved in fsio::list_dir(&self.root)? {
            let moved = self.root.join(&moved);
            if !moved
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(MOVED_PREFIX))
                || !fsio::inspect_subdir(&moved)?
            {
                continue;
            }
            for sessions in fsio::list_dir(&moved)? {
                let sessions = moved.join(&sessions);
                if sessions
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| super::documents::is_moved_variant(n, SESSIONS_DIR))
                    && fsio::inspect_subdir(&sessions)?
                {
                    found.extend(Self::session_files_of(&sessions, id)?);
                }
            }
        }
        Ok(found)
    }

    /// How many legacy files of `id` the pending write's move will carry
    /// to `pre-sqlite-<date>/` after its commit, where the cleanup that
    /// follows deletes them: they are not yet in any archive when the
    /// forget counts, so [`Self::moved_files_of`] misses them. Zero when
    /// the write moves nothing aside.
    fn legacy_files_to_forget(&self, id: &SessionId) -> Result<usize> {
        let sessions = self.root.join(SESSIONS_DIR);
        if !self.aside_pending() || !fsio::inspect_subdir(&sessions)? {
            return Ok(0);
        }
        Ok(Self::session_files_of(&sessions, id)?.len())
    }

    /// `id`'s record and rebuild checkpoint files under
    /// `<sessions>/<yyyy>/<mm>/`.
    fn session_files_of(sessions: &Path, id: &SessionId) -> Result<Vec<PathBuf>> {
        let names = [format!("{id}{RECORD_EXT}"), format!(".{id}.rebuild.md")];
        let mut found = Vec::new();
        for year in fsio::list_dir(sessions)? {
            let year = sessions.join(year);
            if !fsio::inspect_subdir(&year)? {
                continue;
            }
            for month in fsio::list_dir(&year)? {
                let month = year.join(month);
                if !fsio::inspect_subdir(&month)? {
                    continue;
                }
                for name in &names {
                    let path = month.join(name);
                    if fsio::inspect_file(&path)? {
                        found.push(path);
                    }
                }
            }
        }
        Ok(found)
    }

    /// The `s-<n>` of every session, read-only.
    pub fn session_numbers(&self) -> Result<BTreeMap<SessionId, NonZeroU64>> {
        Ok(self.load_sessions()?.numbers)
    }
}

/// Every session table `scribe export --as jsonl` writes (phase 6), with
/// its row order.
const SESSION_EXPORT_TABLES: &[(&str, &str)] = &[
    ("sessions", "id"),
    ("segments", "session, n"),
    ("session_links", "session, position"),
    ("session_terms", "session, position"),
    ("codex_cursors", "session"),
    ("rebuild_checkpoints", "session"),
];

/// Every document table `scribe export --as jsonl` writes (phase 7), with
/// its row order.
const DOC_EXPORT_TABLES: &[(&str, &str)] = &[
    ("entries", "tag, n"),
    ("claims", "n"),
    ("agent_tags", "name"),
    ("marks", "position"),
    ("pickups", "position"),
    ("audit", "id"),
];

/// The checkpoint's file text at the current format, as a row holds it.
pub(super) fn encode_checkpoint(path: &Path, record: &SessionRecord) -> Result<String> {
    String::from_utf8(format::encode_session(path, record)?).map_err(|e| Error::corrupt(path, e))
}

/// `pre-sqlite-<yyyy-mm-dd>`.
pub(super) fn moved_dir_name(now: OffsetDateTime) -> String {
    let date = now.date();
    format!(
        "{MOVED_PREFIX}{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// `path`, or `path.<n>` for the first `n` from 2 that is free, so a second
/// move never replaces what an earlier one kept.
pub(super) fn unused(path: &Path) -> PathBuf {
    if std::fs::symlink_metadata(path).is_err() {
        return path.to_path_buf();
    }
    (2..)
        .map(|n| {
            let mut name = path.as_os_str().to_owned();
            name.push(format!(".{n}"));
            PathBuf::from(name)
        })
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .unwrap_or_else(|| path.to_path_buf())
}

/// Create `target` owner-only, or accept it empty.
fn prepare_target(target: &Path) -> Result<()> {
    match std::fs::symlink_metadata(target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
            }
            fsio::create_private_dir(target)
        }
        Err(e) => Err(Error::io("inspect", target, e)),
        Ok(meta) if meta.is_dir() => {
            if !fsio::list_dir(target)?.is_empty() {
                return Err(refusal(
                    WriteRefusal::ExportTargetExists,
                    format!(
                        "{} is not empty; export into a new or empty directory",
                        target.display()
                    ),
                ));
            }
            Ok(())
        }
        Ok(_) => Err(refusal(
            WriteRefusal::ExportTargetExists,
            format!("{} exists and is not a directory", target.display()),
        )),
    }
}

/// The existing session-show shape, populated solely from a tombstone.
/// It is never listed, exported as a record or offered as evidence.
pub(super) fn forgotten_record(id: &SessionId, at: OffsetDateTime) -> SessionRecord {
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
