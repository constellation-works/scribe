//! Sessions in `scribe.db` (docs/design/scribe/2_design.md §15.7, phase 6):
//! where they are read from, `scribe store migrate`, `scribe export`, and
//! a session's forget.
//!
//! - **Reads.** Every session read comes from the database. A file store
//!   whose sessions never moved in is refused: this build has no file
//!   decoder (phase 8).
//! - **Migrate.** `scribe store migrate` imports the file store's archives
//!   into the archive tables now (`archive.rs`), as the first write does on
//!   its own; it is refused while a session is on, while a rebuild
//!   checkpoint file waits, or when there is nothing to import.
//! - **Locks.** The per-session locks held across model calls (Codex
//!   ingestion, rebuild) are lock files in `locks/`; they hold no data.

use super::db::sessions::{self as rows, SessionTx};
use super::write::Move;
use super::{CONTROL_FILE, Imported, RECORD_EXT, SESSIONS_DIR, Skipped, Store, codec, db, fsio};
use crate::WriteRefusal;
use crate::checks::refusal;
use crate::error::{Error, Result};
use crate::session::{ControlState, SessionId, SessionRecord};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// The directory of advisory lock files under the root.
pub const LOCKS_DIR: &str = "locks";

/// Every session scribe knows, from wherever they live
/// ([`super::repo::Sessions::loaded`]).
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

/// What [`super::repo::Sessions::erase`] erased of a session.
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
pub struct ExportCounts {
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
    /// Every session, read-only, from `scribe.db`; none before the first
    /// write. A file store is refused.
    pub(super) fn load_sessions(&self) -> Result<LoadedSessions> {
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
    pub(super) fn check_stamp(&self) -> Result<()> {
        let path = self.root.join(CONTROL_FILE);
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(());
        }
        codec::check_stamp(&path, &fsio::read(&path)?)
    }

    /// `scribe store migrate`: import the file store's `pre-sqlite-*` and
    /// `pre-issue-*` archives into `scribe.db` in one transaction, then
    /// remove them.
    ///
    /// # Errors
    ///
    /// `migrate_blocked` while any session is switched on, while any rebuild
    /// checkpoint file exists, or when there is nothing to import;
    /// `store_corrupt` for a file store whose records never moved; store
    /// failures.
    pub fn migrate_store(&self, now: OffsetDateTime) -> Result<Imported> {
        self.transaction_as(Move::ByHand, now, || {
            self.with_conn(|_, _| Ok(()))?;
            Ok::<_, Error>(self.moved_now().unwrap_or_default())
        })
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
        let Some(text) = rows::read_checkpoint(&self.root, &original.id)? else {
            return Ok(None);
        };
        let path = rows::db_path(&self.root);
        codec::decode_session(&path, text.as_bytes(), original.id.as_str()).map(Some)
    }

    /// Run `f` in one session transaction on `scribe.db`, inside the store
    /// write it joins or one of its own (the first write moves any files
    /// in first). A write that refuses leaves everything where it was.
    pub(super) fn sessions_tx<T>(&self, f: impl FnOnce(&SessionTx<'_>) -> Result<T>) -> Result<T> {
        self.write_step(|| {
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
    /// `store_corrupt` for a file store whose records never moved into
    /// `scribe.db`; I/O and store failures.
    pub fn export(&self, target: &Path, as_format: ExportFormat) -> Result<ExportCounts> {
        let mut out = ExportCounts::default();
        let rows = match as_format {
            ExportFormat::Markdown => None,
            ExportFormat::Jsonl if !self.root_exists()? => Some(Vec::new()),
            ExportFormat::Jsonl => {
                let db = db::read_export(&self.root, SESSION_EXPORT_TABLES, DOC_EXPORT_TABLES)?;
                let (spooled, _) = self.spooled_audit()?;
                if !db.sessions_moved || !db.docs_moved {
                    self.refuse_file_store()?;
                }
                self.check_stamp()?;
                let mut rows = db.rows;
                if !spooled.is_empty() {
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
                    let bytes = codec::encode_session(&target.join(&rel), record)?;
                    write(rel, &bytes)?;
                }
                let path = target.join(CONTROL_FILE);
                write(
                    CONTROL_FILE.to_owned(),
                    &codec::encode_control(&path, &loaded.control)?,
                )?;
                out.sessions = records.len();
                out.skipped = loaded.skipped.into_iter().map(|(_, s)| s.path).collect();
                self.export_documents(target, &mut write, &mut out)?;
            }
        }
        out.files = files;
        Ok(out)
    }

    /// Erase `id`'s rows, archives, documents and memory in the write under
    /// way, registering it first when it has no `s-<n>` (`number`).
    pub(super) fn erase_session(
        &self,
        id: &SessionId,
        number: Option<NonZeroU64>,
        now: OffsetDateTime,
    ) -> Result<SessionErasure> {
        let (erased, already, changed, files) = self.with_conn(|conn, path| {
            if number.is_none() {
                rows::register(conn, path, id)?;
            }
            let (files, archive_changed) = super::archive::scrub_session(conn, path, id, number)?;
            let (erased, already, mut changed) =
                rows::forget(conn, path, id, now)?.ok_or_else(|| Error::SessionNotFound {
                    id: id.clone(),
                    path: path.to_path_buf(),
                })?;
            changed |= self.forget_session_documents(id, now)?;
            changed |= self.forget_session_memory(id, now)?;
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
    pub(super) fn has_group_membership(&self, id: &SessionId) -> Result<bool> {
        self.with_conn(|conn, path| rows::has_group_membership(conn, path, id))
    }

    /// Change the switches, and write a record, in one session transaction:
    /// `f` changes the state and returns the record to write, if any.
    pub(super) fn change_sessions<T>(
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
    pub(super) fn put_rebuilt(&self, record: &SessionRecord) -> Result<()> {
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
    String::from_utf8(codec::encode_session(path, record)?).map_err(|e| Error::corrupt(path, e))
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
