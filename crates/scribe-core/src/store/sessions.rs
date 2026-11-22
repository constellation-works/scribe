//! Sessions in `scribe.db` (docs/design/scribe/2_design.md §15.7, §15.11,
//! phase 6): where they are read from, how the files move in, and
//! `scribe export`.
//!
//! - **Reads.** Once `record_kinds` holds `sessions`, every session read
//!   comes from the database; before that, from the format-25 files, with
//!   today's decoders (kept for the 30-day window). A file store's `s-<n>`
//!   numbers are the ones the move will give: records by `created`, then
//!   id, and after them the sessions with only a switch, by id.
//! - **The move.** The first write takes the store lock and moves the
//!   sessions in ([`Store::move_sessions`]) in one transaction, checking
//!   the record count, then moves the files to `pre-sqlite-<date>/`, which
//!   scribe never deletes. `control.json` stays as a format stamp with no
//!   sessions, so a format-25 build refuses the store instead of starting a
//!   second set of files. `scribe store migrate` is the same move run by
//!   hand, refused while a session is on, while a rebuild checkpoint waits,
//!   or once the sessions moved.
//! - **Locks.** The per-session locks held across model calls (Codex
//!   ingestion, rebuild) are lock files in `locks/`; they hold no data.

use super::db::sessions::{self as rows, SessionTx};
use super::{
    CONTROL_FILE, RECORD_EXT, SESSIONS_DIR, Skipped, Store, agent::refusal, db, format, fsio,
};
use crate::WriteRefusal;
use crate::error::{Error, Result};
use crate::session::{ControlState, SessionId, SessionRecord};
use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// The directory of per-session lock files under the root.
pub const LOCKS_DIR: &str = "locks";

/// What the moved-aside files directory is called: `pre-sqlite-<date>`.
const MOVED_PREFIX: &str = "pre-sqlite-";

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

/// What a move of the sessions into `scribe.db` did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionsMoved {
    /// Records imported.
    pub sessions: usize,
    /// Switches imported, those of sessions with a record included.
    pub switches: usize,
    /// Codex cursors imported.
    pub cursors: usize,
    /// Rebuild checkpoints imported (only a write's own move imports one;
    /// `store migrate` refuses while one waits).
    pub checkpoints: usize,
    /// Where the files went, relative to the root; `None` when there were
    /// none.
    pub moved_to: Option<String>,
}

/// Which move: a write's own, or `store migrate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Move {
    /// At a write: never refused, so hooks keep working; a rebuild
    /// checkpoint becomes a row.
    AtWrite,
    /// `store migrate`: refused while a session is on or a checkpoint
    /// waits, so no partial record is imported.
    ByHand,
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
            ..Loaded::default()
        };
        for row in found.records {
            match row {
                Ok((_, record)) => loaded.records.push(record),
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

    /// Whether the store still holds session files: a `sessions/`
    /// directory, or a `control.json` from before the move.
    fn has_session_files(&self) -> Result<bool> {
        if fsio::inspect_subdir(&self.root.join(SESSIONS_DIR))? {
            return Ok(true);
        }
        // Moved first: after the move `control.json` is only the stamp.
        let control = self.root.join(CONTROL_FILE);
        Ok(!rows::read_moved(&self.root)? && fsio::inspect_file(&control)?)
    }

    /// At a session write, under the store lock: move the sessions in when
    /// they are still files. A store with none pays a directory check.
    pub(super) fn move_sessions_at_write(&self, now: OffsetDateTime) -> Result<()> {
        if self.has_session_files()? {
            self.move_sessions(Move::AtWrite, now)?;
        }
        Ok(())
    }

    /// `scribe store migrate`: move the sessions into `scribe.db` under the
    /// store lock.
    ///
    /// # Errors
    ///
    /// `migrate_blocked` while any session is switched on, while any rebuild
    /// checkpoint exists, or when the sessions already moved;
    /// `store_corrupt` naming a record that does not read; store failures.
    pub fn migrate_sessions(&self, now: OffsetDateTime) -> Result<SessionsMoved> {
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("store migrate")?;
        self.move_sessions(Move::ByHand, now)
    }

    /// Move the sessions in (§15.11): every record checked and imported in
    /// `created` order, then the switch-only sessions, the Codex cursors
    /// and (at a write) the rebuild checkpoints, in one transaction that
    /// checks the record count; then the files moved aside and
    /// `control.json` rewritten as a stamp. A crash after the commit
    /// leaves files the next write moves aside.
    fn move_sessions(&self, mode: Move, now: OffsetDateTime) -> Result<SessionsMoved> {
        let mut conn = db::open_writer(&self.root, self.lock_wait)?;
        let tx = SessionTx::begin(&mut conn, &self.root)?;
        if tx.moved()? {
            drop(tx);
            if mode == Move::ByHand {
                return Err(refusal(
                    WriteRefusal::MigrateBlocked,
                    "the sessions are already in scribe.db; there is nothing to migrate",
                ));
            }
            let moved_to = self.move_files_aside(now)?;
            return Ok(SessionsMoved {
                moved_to,
                ..SessionsMoved::default()
            });
        }
        let control = self.control_file()?;
        let checkpoints = self.checkpoint_files()?;
        if mode == Move::ByHand {
            if let Some((id, _)) = control.sessions.iter().find(|(_, c)| c.on) {
                return Err(refusal(
                    WriteRefusal::MigrateBlocked,
                    format!(
                        "session {id} is switched on; switch it off (/scribe off) or let its pass settle, then migrate"
                    ),
                ));
            }
            if let Some((id, path)) = checkpoints.first() {
                return Err(refusal(
                    WriteRefusal::MigrateBlocked,
                    format!(
                        "session {id} has a rebuild checkpoint at {}; finish it with `scribe session rebuild {id}` or delete the file, then migrate",
                        path.display()
                    ),
                ));
            }
        }
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
        let has_files = !records.is_empty()
            || !control.sessions.is_empty()
            || !control.codex_cursors.is_empty()
            || fsio::inspect_subdir(&self.root.join(SESSIONS_DIR))?;
        let moved_to = has_files.then(|| moved_dir_name(now));
        tx.mark_moved(now, moved_to.as_deref())?;
        tx.commit()?;
        drop(conn);
        let moved_to = self.move_files_aside(now)?;
        Ok(SessionsMoved {
            sessions: records.len(),
            switches: control.sessions.len(),
            cursors: control.codex_cursors.len(),
            checkpoints: imported_checkpoints,
            moved_to,
        })
    }

    /// Move `sessions/`, `control.json` and the root's Codex lock files to
    /// `pre-sqlite-<date>/`, then write `control.json` as the format stamp.
    /// Moves nothing that is not there, so it finishes an interrupted move.
    fn move_files_aside(&self, now: OffsetDateTime) -> Result<Option<String>> {
        let sessions = self.root.join(SESSIONS_DIR);
        let control = self.root.join(CONTROL_FILE);
        let has_sessions = fsio::inspect_subdir(&sessions)?;
        let old_control = fsio::inspect_file(&control)? && {
            let state = self.control_file()?;
            !state.sessions.is_empty() || !state.codex_cursors.is_empty()
        };
        let locks: Vec<String> = fsio::list_dir(&self.root)?
            .into_iter()
            .filter(|n| n.starts_with(".codex-") && n.ends_with(".lock"))
            .collect();
        let mut moved_to = None;
        if has_sessions || old_control || !locks.is_empty() {
            let name = moved_dir_name(now);
            let dir = self.root.join(&name);
            self.prepare_subdir(&dir)?;
            if has_sessions {
                fsio::rename(&sessions, &unused(&dir.join(SESSIONS_DIR)))?;
            }
            if old_control {
                fsio::rename(&control, &unused(&dir.join(CONTROL_FILE)))?;
            }
            for lock in locks {
                fsio::rename(&self.root.join(&lock), &unused(&dir.join(&lock)))?;
            }
            moved_to = Some(name);
        }
        self.stamp_format()?;
        Ok(moved_to)
    }

    /// Rewrite `control.json` at the current format with no sessions: the
    /// switches live in `scribe.db`, and the stamp makes an older build
    /// refuse the store (STD-03 R10).
    pub(super) fn stamp_format(&self) -> Result<()> {
        let path = self.root.join(CONTROL_FILE);
        fsio::write_atomic(
            &path,
            &format::encode_control(&path, &ControlState::default())?,
        )
    }

    /// Every rebuild checkpoint file of a file store,
    /// `sessions/<yyyy>/<mm>/.<id>.rebuild.md`, by session.
    fn checkpoint_files(&self) -> Result<Vec<(SessionId, PathBuf)>> {
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

    /// The session's lock file under `locks/`, the directory created
    /// owner-only when missing.
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

    /// Run `f` in one session transaction on `scribe.db`, under the lock
    /// the caller holds. The first session write moves any session files
    /// in first; a store that never had any is marked moved here, with its
    /// format stamp. A write that refuses, or one that never touches a
    /// session, leaves the files where they are.
    pub(super) fn sessions_tx<T>(&self, f: impl FnOnce(&SessionTx<'_>) -> Result<T>) -> Result<T> {
        self.move_sessions_at_write(OffsetDateTime::now_utc())?;
        let mut conn = db::open_writer(&self.root, self.lock_wait)?;
        let tx = SessionTx::begin(&mut conn, &self.root)?;
        self.check_stamp()?;
        let fresh = !tx.moved()?;
        if fresh {
            tx.mark_moved(OffsetDateTime::now_utc(), None)?;
        }
        let out = f(&tx)?;
        tx.commit()?;
        if fresh {
            self.stamp_format()?;
        }
        Ok(out)
    }

    /// `scribe export`: write every session to `target`, a directory that
    /// is missing or empty, created owner-only. Reads the store only: no
    /// lock, no store write (STD-01 R31).
    ///
    /// # Errors
    ///
    /// `export_target_exists` for a directory with anything in it;
    /// `export_needs_migrate` for JSONL of sessions still in files; I/O
    /// and store failures.
    pub fn export_sessions(&self, target: &Path, as_format: ExportFormat) -> Result<Exported> {
        let mut out = Exported::default();
        let rows = match as_format {
            ExportFormat::Markdown => None,
            ExportFormat::Jsonl if !self.root_exists()? => Some(Vec::new()),
            ExportFormat::Jsonl => match rows::read_export(&self.root)? {
                Some(rows) => Some(rows),
                None if !self.has_session_files()? => Some(Vec::new()),
                None => {
                    return Err(refusal(
                        WriteRefusal::ExportNeedsMigrate,
                        "JSONL export reads scribe.db, and this store's sessions are still files; run `scribe store migrate`, or export --as markdown",
                    ));
                }
            },
        };
        prepare_target(target)?;
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
            out.files.push(rel);
            Ok(())
        };
        match rows {
            Some(rows) => {
                let mut tables: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
                for (table, row) in rows {
                    let line = tables.entry(table).or_default();
                    serde_json::to_writer(&mut *line, &row)
                        .map_err(|e| Error::io("encode", target, std::io::Error::other(e)))?;
                    line.push(b'\n');
                }
                let sessions = tables
                    .get("sessions")
                    .map_or(0, |b| b.iter().filter(|c| **c == b'\n').count());
                for (table, bytes) in tables {
                    write(format!("{table}.jsonl"), &bytes)?;
                }
                out.sessions = sessions;
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
            }
        }
        Ok(out)
    }

    /// The `s-<n>` of every session, read-only.
    pub fn session_numbers(&self) -> Result<BTreeMap<SessionId, NonZeroU64>> {
        Ok(self.load_sessions()?.numbers)
    }
}

/// The checkpoint's file text at the current format, as a row holds it.
pub(super) fn encode_checkpoint(path: &Path, record: &SessionRecord) -> Result<String> {
    String::from_utf8(format::encode_session(path, record)?).map_err(|e| Error::corrupt(path, e))
}

/// `pre-sqlite-<yyyy-mm-dd>`.
fn moved_dir_name(now: OffsetDateTime) -> String {
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
fn unused(path: &Path) -> PathBuf {
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
