//! Entries, claims, store tags, marks, pickups and the audit log in
//! `scribe.db` (docs/design/scribe/2_design.md §15.7, §15.11, phase 7):
//! where they are read from, how their files move in, and their part of
//! `scribe export`.
//!
//! - **Reads.** Once `record_kinds` holds the document kinds, every read
//!   comes from the database; before that, from the format-26 files, with
//!   today's decoders (kept for the 30-day window). Reads take no lock and
//!   write nothing (STD-01 R31).
//! - **The move.** [`Store::import_at_write`] runs at the start of the first
//!   write's transaction: the sessions (phase 6), then these kinds, every
//!   file checked before any row is written, each kind's row count checked
//!   against its files. Audit lines are imported with their own format; a
//!   torn line is counted and reported, as a read of the log does.
//! - **Stamps.** After the move, `control.json`, `proposals.json`,
//!   `tags.json`, `marks.json` and `pickups.json` are empty documents at
//!   the current format, so an older build refuses the store.

use super::audit::{self, AuditEvent};
use super::db::{self, Conn as Connection, DB_FILE, docs};
use super::write::Move;
use super::{
    AUDIT_FILE, CONTROL_FILE, ENTRIES_DIR, MARKS_FILE, PICKUPS_FILE, PROPOSALS_FILE, RECORD_EXT,
    SESSIONS_DIR, Skipped, Store, format, fsio, visible,
};
use crate::action::Pickup;
use crate::annotation::MarkRecord;
use crate::entry::{Entry, EntryId};
use crate::error::{Error, Result};
use crate::proposal::Proposal;
use crate::session::{ControlState, SessionId};
use crate::tag::StoreTag;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// The store tags' file, before the move.
pub(super) const TAGS_FILE: &str = "tags.json";

/// Advisory lock under `locks/` serializing post-commit read-modify-writes of
/// archived `pre-sqlite-*` proposal and audit files during entry forget
/// (STD-03 R6).
pub(super) const ARCHIVE_FORGET_LOCK: &str = "archive-forget.lock";

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ForgetMovedStep {
    /// At entry to `forget_moved_entry`, after the SQLite transaction commits
    /// and before taking the archive-forget lock.
    BeforeLock,
    /// Inside `forget_moved_proposals`, after reading the archived file and
    /// before writing the updated file.
    AfterProposalsRead,
    /// Inside `forget_moved_audit`, after reading the archived file and before
    /// writing the updated file.
    AfterAuditRead,
}

#[cfg(test)]
type ForgetMovedHook = Box<dyn FnMut(ForgetMovedStep)>;

#[cfg(test)]
thread_local! {
    /// Tests: observe or pause steps inside `forget_moved_entry` to drive a
    /// deterministic interleaving between two independently committed forgets.
    static FORGET_MOVED_HOOK: std::cell::RefCell<Option<ForgetMovedHook>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) struct ForgetMovedHookGuard;

#[cfg(test)]
impl Drop for ForgetMovedHookGuard {
    fn drop(&mut self) {
        FORGET_MOVED_HOOK.with(|h| {
            h.borrow_mut().take();
        });
    }
}

#[cfg(test)]
pub(super) fn install_forget_moved_hook(
    hook: impl FnMut(ForgetMovedStep) + 'static,
) -> ForgetMovedHookGuard {
    FORGET_MOVED_HOOK.with(|h| {
        *h.borrow_mut() = Some(Box::new(hook));
    });
    ForgetMovedHookGuard
}

#[cfg(test)]
fn notify_forget_moved(step: ForgetMovedStep) {
    let mut hook = FORGET_MOVED_HOOK.with(|h| h.borrow_mut().take());
    if let Some(f) = hook.as_mut() {
        f(step);
    }
    FORGET_MOVED_HOOK.with(|h| {
        if h.borrow().is_none() {
            *h.borrow_mut() = hook;
        }
    });
}

/// What `scribe store migrate`, or a write's own move, moved into
/// `scribe.db`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Moved {
    /// Session records imported.
    pub sessions: usize,
    /// Switches imported, those of sessions with a record included.
    pub switches: usize,
    /// Codex cursors imported.
    pub cursors: usize,
    /// Rebuild checkpoints imported (only a write's own move imports one;
    /// `store migrate` refuses while one waits).
    pub checkpoints: usize,
    /// Entries imported.
    pub entries: usize,
    /// Claims imported.
    pub claims: usize,
    /// Store tags imported.
    pub tags: usize,
    /// Annotation marks imported.
    pub marks: usize,
    /// Pickups imported.
    pub pickups: usize,
    /// Audit lines imported, spooled ones included.
    pub audit_lines: usize,
    /// Audit lines that did not parse, so were not imported.
    pub audit_torn: usize,
    /// Where the files went, relative to the root; `None` until they move,
    /// or when there were none.
    pub moved_to: Option<String>,
}

impl Store {
    /// Whether entries, claims, tags, marks, pickups and the audit log
    /// live in `scribe.db`. Reads only.
    pub(super) fn documents_moved(&self) -> Result<bool> {
        if !self.root_exists()? {
            return Ok(false);
        }
        Ok(docs::read(&self.root, |_, _| Ok(()))?.is_some())
    }

    /// Run `f` on the document tables when the documents live in the
    /// database (after checking the format stamp), else `None`.
    pub(super) fn read_db<T>(
        &self,
        f: impl Fn(&Connection, &Path) -> Result<T>,
    ) -> Result<Option<T>> {
        if !self.root_exists()? {
            return Ok(None);
        }
        let found = docs::read(&self.root, f)?;
        if found.is_some() {
            self.check_stamp()?;
        }
        Ok(found)
    }

    /// Every entry, plus those that did not read. Anything other than a
    /// parse failure (a link, loose modes, a newer format, an I/O error)
    /// refuses the whole load.
    pub(super) fn load_entries(&self) -> Result<(Vec<Entry>, Vec<Skipped>)> {
        let Some(rows) = self.read_db(docs::entries)? else {
            return self.load_entry_files();
        };
        let mut entries = Vec::new();
        let mut skipped = Vec::new();
        for row in rows {
            match row {
                Ok(entry) => entries.push(entry),
                Err(bad) => skipped.push(Skipped {
                    path: bad.path,
                    error: bad.error,
                }),
            }
        }
        Ok((entries, skipped))
    }

    /// Erase session handoffs and withdraw claims in the same write.
    pub(super) fn forget_session_documents(
        &self,
        id: &SessionId,
        now: OffsetDateTime,
    ) -> Result<bool> {
        let (entries, skipped) = self.load_entries()?;
        if let Some(bad) = skipped.into_iter().next() {
            return Err(bad.error);
        }
        let number = self.session_numbers()?.get(id).map(|n| format!("s-{n}"));
        let matches = |value: &str| value == id.as_str() || number.as_deref() == Some(value);
        let mut changed = false;
        let mut removed = Vec::new();
        for before in entries {
            let mut entry = before.clone();
            if handoff_names(&entry, matches) {
                self.with_conn(|conn, db| docs::delete_handoff(conn, db, &entry.id))?;
                removed.push(entry.id);
            } else {
                detach_entry(&mut entry, matches, now);
                if entry != before {
                    self.write_entry(&entry, false)?;
                    if entry.author.kind == crate::AuthorKind::Scribe {
                        removed.push(entry.id);
                    }
                    changed = true;
                }
            }
        }
        let before = self.proposals()?;
        let mut after = before.clone();
        detach_claims(&mut after, matches, &removed, now);
        if before != after {
            self.write_proposals(&after)?;
            changed = true;
        }
        Ok(changed || !removed.is_empty())
    }

    /// The entry with this id.
    pub fn entry(&self, id: &EntryId) -> Result<Entry> {
        if let Some(rows) = self.read_db(docs::entries)? {
            let path = self.root.join(DB_FILE);
            for row in rows {
                match row {
                    Ok(entry) if &entry.id == id => return Ok(entry),
                    Err(bad) if bad.id.as_ref() == Some(id) => return Err(bad.error),
                    Ok(_) | Err(_) => {}
                }
            }
            return Err(Error::EntryNotFound {
                id: id.clone(),
                path,
            });
        }
        let path = self.entry_path(id);
        let not_found = || Error::EntryNotFound {
            id: id.clone(),
            path: path.clone(),
        };
        let tag_dir = self.root.join(ENTRIES_DIR).join(id.tag().as_str());
        if !self.root_exists()?
            || !fsio::inspect_subdir(&self.root.join(ENTRIES_DIR))?
            || !fsio::inspect_subdir(&tag_dir)?
            || !fsio::inspect_file(&path)?
        {
            return Err(not_found());
        }
        format::decode_entry(
            &path,
            &fsio::read(&path)?,
            &id.to_string(),
            id.tag().as_str(),
        )
    }

    /// Every entry file of a file store, plus the files that did not parse.
    fn load_entry_files(&self) -> Result<(Vec<Entry>, Vec<Skipped>)> {
        let mut entries = Vec::new();
        let mut skipped = Vec::new();
        let dir = self.root.join(ENTRIES_DIR);
        if !self.root_exists()? || !fsio::inspect_subdir(&dir)? {
            return Ok((entries, skipped));
        }
        for tag in visible(fsio::list_dir(&dir)?) {
            let tag_dir = dir.join(&tag);
            if !fsio::inspect_subdir(&tag_dir)? {
                continue;
            }
            for name in visible(fsio::list_dir(&tag_dir)?) {
                let Some(stem) = name.strip_suffix(RECORD_EXT) else {
                    continue;
                };
                let path = tag_dir.join(&name);
                if !fsio::inspect_file(&path)? {
                    continue;
                }
                match format::decode_entry(&path, &fsio::read(&path)?, stem, &tag) {
                    Ok(entry) => entries.push(entry),
                    Err(error @ Error::Corrupt { .. }) => skipped.push(Skipped { path, error }),
                    Err(other) => return Err(other),
                }
            }
        }
        Ok((entries, skipped))
    }

    /// Every proposal, by id; none when there are none.
    pub fn proposals(&self) -> Result<Vec<Proposal>> {
        let mut proposals = match self.read_db(docs::claims)? {
            Some(claims) => claims,
            None => self.proposal_file()?,
        };
        proposals.sort_by_key(|p| p.id);
        Ok(proposals)
    }

    fn proposal_file(&self) -> Result<Vec<Proposal>> {
        let path = self.root.join(PROPOSALS_FILE);
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(Vec::new());
        }
        format::decode_proposals(&path, &fsio::read(&path)?)
    }

    /// The store tags (`scribe tag add`), without writing.
    pub fn store_tags(&self) -> Result<Vec<StoreTag>> {
        if let Some(tags) = self.read_db(docs::tags)? {
            return Ok(tags);
        }
        let path = self.root.join(TAGS_FILE);
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(Vec::new());
        }
        format::decode_tags(&path, &fsio::read(&path)?)
    }

    /// Every pickup, oldest first; none for a missing store. Reads only.
    pub fn pickups(&self) -> Result<Vec<Pickup>> {
        if let Some(pickups) = self.read_db(docs::pickups)? {
            return Ok(pickups);
        }
        let path = self.root.join(PICKUPS_FILE);
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(Vec::new());
        }
        format::decode_pickups(&path, &fsio::read(&path)?)
    }

    /// Every marker outcome, in order.
    pub(super) fn all_marks(&self) -> Result<Vec<MarkRecord>> {
        if let Some(marks) = self.read_db(docs::marks)? {
            return Ok(marks);
        }
        let path = self.root.join(MARKS_FILE);
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(Vec::new());
        }
        format::decode_marks(&path, &fsio::read(&path)?)
    }

    /// The audit log's events, oldest first, the spooled ones last, and how
    /// many lines did not parse. Reads only: no lock, no write; a store with
    /// none has none.
    ///
    /// # Errors
    ///
    /// An unsafe file (a link, loose modes) as for any store read, or a
    /// line of a newer audit format (`store_too_new`).
    pub fn audit_events(&self) -> Result<(Vec<AuditEvent>, usize)> {
        let (lines, torn) = self.audit_lines()?;
        Ok((lines.into_iter().map(|(_, event)| event).collect(), torn))
    }

    /// Audit lines with their original formats, in the same order as `audit_events`.
    fn audit_lines(&self) -> Result<(Vec<(u32, AuditEvent)>, usize)> {
        if !self.root_exists()? {
            return Ok((Vec::new(), 0));
        }
        let (mut lines, mut torn, drained) = match self.read_db(|conn, path| {
            let drained = docs::spooled_names(conn, path)?;
            Ok((docs::audit(conn, path)?, drained))
        })? {
            Some((lines, drained)) => (lines, 0, drained),
            None => {
                let (lines, torn) = self.audit_file()?;
                (lines, torn, Vec::new())
            }
        };
        let (spooled, spool_torn) = self.spooled_audit()?;
        torn += spool_torn;
        lines.extend(
            spooled
                .into_iter()
                .filter(|(name, _, _)| !drained.contains(name))
                .map(|(_, format, event)| (format, event)),
        );
        Ok((lines, torn))
    }

    /// `audit.jsonl`'s lines with their formats, and how many did not parse.
    fn audit_file(&self) -> Result<(Vec<(u32, AuditEvent)>, usize)> {
        let (lines, torn) = self.audit_file_for_import()?;
        Ok((
            lines.into_iter().map(|l| (l.format, l.event)).collect(),
            torn,
        ))
    }

    /// `audit.jsonl`'s lines with their formats and idempotency keys, and
    /// how many did not parse.
    fn audit_file_for_import(&self) -> Result<(Vec<audit::ImportedLine>, usize)> {
        let path = self.root.join(AUDIT_FILE);
        if !fsio::inspect_file(&path)? {
            return Ok((Vec::new(), 0));
        }
        audit::decode_for_import(&path, &fsio::read(&path)?)
    }

    /// Write `entry` in the write under way. A new entry never replaces
    /// one: ids come from the listing, so a taken id means the listing
    /// missed it, and the write is refused.
    pub(super) fn write_entry(&self, entry: &Entry, new: bool) -> Result<()> {
        self.with_conn(|conn, path| {
            docs::put_entry(conn, path, entry, new, |e| {
                db::db_error("write entries to", path, e)
            })
        })
    }

    /// Write the proposals that changed, in the write under way.
    pub(super) fn write_proposals(&self, proposals: &[Proposal]) -> Result<()> {
        self.with_conn(|conn, path| {
            let before = docs::claims(conn, path)?;
            docs::put_claims(conn, path, &before, proposals, |e| {
                db::db_error("write claims to", path, e)
            })
        })
    }

    /// Replace the store tags, in the write under way.
    pub(super) fn write_tags(&self, tags: &[StoreTag]) -> Result<()> {
        self.with_conn(|conn, path| {
            docs::put_tags(conn, path, tags, |e| db::db_error("write tags to", path, e))
        })
    }

    /// Replace the marks, in the write under way.
    pub(super) fn write_marks(&self, marks: &[MarkRecord]) -> Result<()> {
        self.with_conn(|conn, path| {
            docs::put_marks(conn, path, marks, |e| {
                db::db_error("write marks to", path, e)
            })
        })
    }

    /// Replace the pickups, in the write under way.
    pub(super) fn write_pickups(&self, pickups: &[Pickup]) -> Result<()> {
        self.with_conn(|conn, path| {
            docs::put_pickups(conn, path, pickups, |e| {
                db::db_error("write pickups to", path, e)
            })
        })
    }

    /// At the start of a write's transaction: move every kind still in
    /// files into the database, convert any issue records, and drain the
    /// spooled audit lines. `store migrate` (`mode`) is refused first when
    /// there is nothing to move or the move is unsafe.
    pub(super) fn import_at_write(&self, mode: Move, now: OffsetDateTime) -> Result<Moved> {
        let conn = db::open_writer(&self.root, self.lock_wait)?;
        let path = self.root.join(DB_FILE);
        if mode == Move::ByHand {
            self.refuse_migrate(&conn)?;
        }
        let mut moved = self.import_sessions(mode, now)?;
        let fail = |e| db::db_error("move documents into", &path, e);
        if !docs::moved(&conn).map_err(fail)? {
            self.import_documents(&conn, &path, &mut moved)?;
            // Each kind records the directory its files move to, or none
            // when it has no files (`record_kinds.moved_to`).
            let with_files = self.document_files_to_move_aside()?;
            let archive = super::sessions::moved_dir_name(now);
            docs::mark_moved(&conn, now, |kind| {
                with_files
                    .iter()
                    .any(|(k, _)| *k == kind)
                    .then(|| archive.clone())
            })
            .map_err(fail)?;
            self.after(|a| a.aside = true);
        } else if self.audit_file_present()? {
            // A line an older build appended after the move, or the
            // original log left by an interruption before `move_files_aside`:
            // keyed so already-imported lines are not added twice.
            let (lines, torn) = self.audit_file_for_import()?;
            for line in &lines {
                if docs::put_audit(
                    &conn,
                    &path,
                    line.format,
                    &line.event,
                    Some(&line.key),
                    fail,
                )? {
                    moved.audit_lines += 1;
                }
            }
            moved.audit_torn += torn;
            self.after(|a| a.aside = true);
        }
        if self.has_files_to_move_aside()? {
            self.after(|a| a.aside = true);
        }
        if docs::has_pending_archive_forgets(&conn).map_err(fail)? {
            self.after(|a| a.archive_forgets = true);
            db::scope::Scope::truncate_later(&self.root);
        }
        moved.audit_lines += self.drain_spool(&conn, &path)?;
        self.convert_issues(now)?;
        Ok(moved)
    }

    /// Import entries, claims, tags, marks, pickups and audit lines from
    /// their files, each kind's rows counted against its files. Every file
    /// must read first: one that does not is the user's to fix or remove.
    fn import_documents(&self, conn: &Connection, path: &Path, moved: &mut Moved) -> Result<()> {
        let fail = |e| db::db_error("move documents into", path, e);
        let (mut entries, skipped) = self.load_entry_files()?;
        if let Some(bad) = skipped.into_iter().next() {
            return Err(bad.error);
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        for entry in &entries {
            docs::put_entry(conn, path, entry, true, fail)?;
        }
        let claims = self.proposal_file()?;
        docs::put_claims(conn, path, &[], &claims, fail)?;
        let tags = {
            let file = self.root.join(TAGS_FILE);
            if fsio::inspect_file(&file)? {
                format::decode_tags(&file, &fsio::read(&file)?)?
            } else {
                Vec::new()
            }
        };
        docs::put_tags(conn, path, &tags, fail)?;
        let marks = {
            let file = self.root.join(MARKS_FILE);
            if fsio::inspect_file(&file)? {
                format::decode_marks(&file, &fsio::read(&file)?)?
            } else {
                Vec::new()
            }
        };
        docs::put_marks(conn, path, &marks, fail)?;
        let pickups = {
            let file = self.root.join(PICKUPS_FILE);
            if fsio::inspect_file(&file)? {
                format::decode_pickups(&file, &fsio::read(&file)?)?
            } else {
                Vec::new()
            }
        };
        docs::put_pickups(conn, path, &pickups, fail)?;
        let (lines, torn) = self.audit_file_for_import()?;
        if torn > 0 {
            tracing::warn!(
                torn,
                "audit.jsonl lines that do not parse were not moved into scribe.db"
            );
        }
        for line in &lines {
            docs::put_audit(conn, path, line.format, &line.event, Some(&line.key), fail)?;
        }
        let counts = [
            ("entries", entries.len()),
            ("claims", claims.len()),
            ("agent_tags", tags.len()),
            ("marks", marks.len()),
            ("pickups", pickups.len()),
            ("audit", lines.len()),
        ];
        for (table, files) in counts {
            if docs::count(conn, table).map_err(fail)? != files {
                return Err(Error::corrupt(
                    path,
                    format!(
                        "the move found {files} {table} in files but scribe.db holds a different count"
                    ),
                ));
            }
        }
        moved.entries = entries.len();
        moved.claims = claims.len();
        moved.tags = tags.len();
        moved.marks = marks.len();
        moved.pickups = pickups.len();
        moved.audit_lines += lines.len();
        moved.audit_torn += torn;
        Ok(())
    }

    fn audit_file_present(&self) -> Result<bool> {
        fsio::inspect_file(&self.root.join(AUDIT_FILE))
    }

    /// Whether files that now live in the database are still in the root:
    /// an interrupted move left them.
    pub(super) fn has_files_to_move_aside(&self) -> Result<bool> {
        if fsio::inspect_subdir(&self.root.join(SESSIONS_DIR))?
            || fsio::inspect_subdir(&self.root.join(ENTRIES_DIR))?
            || self.audit_file_present()?
        {
            return Ok(true);
        }
        for name in [PROPOSALS_FILE, TAGS_FILE, MARKS_FILE, PICKUPS_FILE] {
            if self.document_has_content(name)? {
                return Ok(true);
            }
        }
        self.control_has_content()
    }

    /// Whether a JSON document holds anything: an empty one is a stamp.
    fn document_has_content(&self, name: &str) -> Result<bool> {
        let path = self.root.join(name);
        if !fsio::inspect_file(&path)? {
            return Ok(false);
        }
        let bytes = fsio::read(&path)?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| Error::corrupt(&path, e))?;
        Ok(value
            .as_object()
            .is_some_and(|o| o.iter().any(|(k, v)| k != "format" && !is_empty(v))))
    }

    fn control_has_content(&self) -> Result<bool> {
        let path = self.root.join(CONTROL_FILE);
        if !fsio::inspect_file(&path)? {
            return Ok(false);
        }
        let state = self.control_file()?;
        Ok(!state.sessions.is_empty() || !state.codex_cursors.is_empty())
    }

    /// Files and directories under the root that `move_files_aside` will
    /// rename into `pre-sqlite-<date>/`: the session files, then the
    /// document files.
    fn files_to_move_aside(&self) -> Result<Vec<String>> {
        let mut moves = self.session_files_to_move_aside()?;
        moves.extend(
            self.document_files_to_move_aside()?
                .into_iter()
                .map(|(_, name)| name),
        );
        Ok(moves)
    }

    /// The session files `move_files_aside` renames: `sessions/`,
    /// `control.json` while it holds a switch or a Codex cursor, and the
    /// root's `.codex-<id>.lock` files. Empty when the `sessions` record
    /// kind had no files, so its `moved_to` is `None`.
    pub(super) fn session_files_to_move_aside(&self) -> Result<Vec<String>> {
        let mut moves = Vec::new();
        if fsio::inspect_subdir(&self.root.join(SESSIONS_DIR))? {
            moves.push(SESSIONS_DIR.to_owned());
        }
        if self.control_has_content()? {
            moves.push(CONTROL_FILE.to_owned());
        }
        moves.extend(
            fsio::list_dir(&self.root)?
                .into_iter()
                .filter(|n| n.starts_with(".codex-") && n.ends_with(".lock")),
        );
        Ok(moves)
    }

    /// The document files `move_files_aside` renames, each with the record
    /// kind its rows came from: `entries/`, and each JSON document or the
    /// audit log that holds anything.
    fn document_files_to_move_aside(&self) -> Result<Vec<(&'static str, String)>> {
        let mut moves = Vec::new();
        if fsio::inspect_subdir(&self.root.join(ENTRIES_DIR))? {
            moves.push(("entries", ENTRIES_DIR.to_owned()));
        }
        for (kind, name) in [
            ("claims", PROPOSALS_FILE),
            ("agent_tags", TAGS_FILE),
            ("marks", MARKS_FILE),
            ("pickups", PICKUPS_FILE),
        ] {
            if self.document_has_content(name)? {
                moves.push((kind, name.to_owned()));
            }
        }
        if self.audit_file_present()? {
            moves.push(("audit", AUDIT_FILE.to_owned()));
        }
        Ok(moves)
    }

    /// Whether the pending write will create `pre-sqlite-<date>/` when its
    /// post-commit cleanup runs.
    pub(super) fn will_move_aside(&self) -> Result<bool> {
        Ok(self.aside_pending() && !self.files_to_move_aside()?.is_empty())
    }

    /// Move every file that now lives in the database to
    /// `pre-sqlite-<date>/` (sessions, entries, the JSON documents, the
    /// audit log, the root's Codex lock files), then write the stamps.
    /// Moves nothing that is not there, so it finishes an interrupted move.
    pub(super) fn move_files_aside(&self, now: OffsetDateTime) -> Result<Option<String>> {
        let moves = self.files_to_move_aside()?;
        let mut moved_to = None;
        if !moves.is_empty() {
            let name = super::sessions::moved_dir_name(now);
            let dir = self.root.join(&name);
            self.prepare_subdir(&dir)?;
            for item in moves {
                fsio::rename(
                    &self.root.join(&item),
                    &super::sessions::unused(&dir.join(&item)),
                )?;
            }
            moved_to = Some(name);
        }
        self.write_stamps()?;
        Ok(moved_to)
    }

    /// Rewrite the format stamps, or note it for after the write under way.
    pub(super) fn stamp_format(&self) -> Result<()> {
        if self.after(|a| a.stamp = true) {
            return Ok(());
        }
        self.write_stamps()
    }

    /// `control.json`, `proposals.json`, `tags.json`, `marks.json` and
    /// `pickups.json` at the current format with nothing in them: what
    /// they held lives in `scribe.db`, and each makes an older build that
    /// reads it refuse the store (STD-03 R10).
    fn write_stamps(&self) -> Result<()> {
        let control = self.root.join(CONTROL_FILE);
        fsio::write_atomic(
            &control,
            &format::encode_control(&control, &ControlState::default())?,
        )?;
        let proposals = self.root.join(PROPOSALS_FILE);
        fsio::write_atomic(&proposals, &format::encode_proposals(&proposals, &[])?)?;
        let tags = self.root.join(TAGS_FILE);
        fsio::write_atomic(&tags, &format::encode_tags(&tags, &[])?)?;
        let marks = self.root.join(MARKS_FILE);
        fsio::write_atomic(&marks, &format::encode_marks(&marks, &[])?)?;
        let pickups = self.root.join(PICKUPS_FILE);
        fsio::write_atomic(&pickups, &format::encode_pickups(&pickups, &[])?)
    }

    /// `scribe export --as markdown` of the documents: one file per entry,
    /// `proposals.json`, `tags.json`, `marks.json`, `pickups.json` and
    /// `audit.jsonl`, the shapes of today's files, so the export is a file
    /// store this build reads and `store migrate` moves back in.
    pub(super) fn export_documents(
        &self,
        target: &Path,
        write: &mut dyn FnMut(String, &[u8]) -> Result<()>,
        out: &mut super::Exported,
    ) -> Result<()> {
        let (mut entries, skipped) = self.load_entries()?;
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        for entry in &entries {
            let rel = format!(
                "{ENTRIES_DIR}/{}/{}{RECORD_EXT}",
                entry.tag.as_str(),
                entry.id
            );
            let bytes = format::encode_entry(&target.join(&rel), entry)?;
            write(rel, &bytes)?;
        }
        out.entries = entries.len();
        out.skipped.extend(skipped.into_iter().map(|s| s.path));
        let proposals = self.proposals()?;
        let path = target.join(PROPOSALS_FILE);
        write(
            PROPOSALS_FILE.to_owned(),
            &format::encode_proposals(&path, &proposals)?,
        )?;
        out.claims = proposals.len();
        let path = target.join(TAGS_FILE);
        write(
            TAGS_FILE.to_owned(),
            &format::encode_tags(&path, &self.store_tags()?)?,
        )?;
        let path = target.join(MARKS_FILE);
        write(
            MARKS_FILE.to_owned(),
            &format::encode_marks(&path, &self.all_marks()?)?,
        )?;
        let path = target.join(PICKUPS_FILE);
        write(
            PICKUPS_FILE.to_owned(),
            &format::encode_pickups(&path, &self.pickups()?)?,
        )?;
        let (lines, _) = self.audit_lines()?;
        let mut log = Vec::new();
        for (format, event) in &lines {
            log.extend(audit::encode_line(*format, event).map_err(|e| {
                Error::corrupt(
                    target.join(AUDIT_FILE),
                    format!("an audit line does not encode: {e}"),
                )
            })?);
        }
        write(AUDIT_FILE.to_owned(), &log)?;
        out.audit_lines = lines.len();
        Ok(())
    }

    /// Whether any `pre-sqlite-*` moved-aside archive directory exists under
    /// the store root.
    fn has_moved_archives(&self) -> Result<bool> {
        if !self.root_exists()? {
            return Ok(false);
        }
        Ok(fsio::list_dir(&self.root)?
            .into_iter()
            .any(|name| name.starts_with(super::sessions::MOVED_PREFIX)))
    }

    /// Delete the stored messages cited by `entry` and durably record the
    /// archive-erasure obligation inside the forget's transaction so
    /// post-commit cleanup (or a restarted write after a crash) finishes
    /// erasing `pre-sqlite-*` files without persisting cleartext (STD-03 R8).
    pub(super) fn forget_entry_residue(
        &self,
        entry: &Entry,
        proposals: &[Proposal],
        now: OffsetDateTime,
    ) -> Result<()> {
        // The cited turns' stored messages go first: they hold the words
        // the forget removes, and deleting them again is a no-op.
        self.forget_cited_messages(entry, proposals)?;
        if !self.has_moved_archives()? && !self.aside_pending() {
            return Ok(());
        }
        let texts = erased_texts(entry, proposals);
        let salt = forget_salt(&entry.id, &self.root);
        let digests = digest_texts(&salt, &texts);
        self.with_conn(|conn, path| {
            docs::put_pending_archive_forget(conn, path, &entry.id, &salt, &digests, now, |e| {
                db::db_error("record pending archive forget in", path, e)
            })
        })?;
        let queued = self.after(|after| {
            after.archive_forgets = true;
        });
        if !queued {
            self.apply_pending_archive_forgets()?;
        }
        Ok(())
    }

    /// Drain every committed entry-forget archive erasure from
    /// `pending_archive_forgets` (STD-03 R8–R9). Runs after the transaction
    /// commits, serialized across processes by `locks/archive-forget.lock`,
    /// and deletes the obligation rows only after every `pre-sqlite-*` archive
    /// is cleaned. Never holds the archive lock and a SQLite write transaction
    /// at the same time (STD-03 R3).
    pub(super) fn apply_pending_archive_forgets(&self) -> Result<()> {
        #[cfg(test)]
        if super::write::CRASH_BEFORE_ARCHIVE_FORGET.with(|c| c.replace(false)) {
            return Err(Error::io(
                "clean moved archive in",
                self.root.join(DB_FILE),
                std::io::Error::other(
                    "a crash injected by a test before apply_pending_archive_forgets",
                ),
            ));
        }
        if !self.root_exists()? {
            return Ok(());
        }
        let path = self.root.join(DB_FILE);
        let conn = db::open_new_writer(&self.root, self.lock_wait)?;
        let pending = docs::pending_archive_forgets(&conn, &path)?;
        if pending.is_empty() {
            return Ok(());
        }
        if self.has_moved_archives()? {
            #[cfg(test)]
            notify_forget_moved(ForgetMovedStep::BeforeLock);
            let lock_path = self.lock_path(ARCHIVE_FORGET_LOCK)?;
            let lock = fsio::LockGuard::acquire(&lock_path, "forget moved entry", self.lock_wait)?;
            for item in &pending {
                self.forget_moved_entry(item, &lock)?;
            }
            drop(lock);
        }
        let ids: Vec<i64> = pending.iter().map(|p| p.id).collect();
        let tx = db::scope::Tx::begin(&conn, &path, "clear pending archive forgets in")?;
        docs::delete_pending_archive_forgets(&tx, &ids, |e| tx.fail(e))?;
        tx.commit()
    }

    /// Plan erasure of an archived session's structured references. The
    /// caller recomputes the plan under the archive lock before writing.
    pub(super) fn moved_session_documents(
        &self,
        id: &SessionId,
        number: Option<std::num::NonZeroU64>,
        now: OffsetDateTime,
    ) -> Result<Vec<(PathBuf, Option<Vec<u8>>)>> {
        let number = number.map(|n| format!("s-{n}"));
        let matches = |value: &str| value == id.as_str() || number.as_deref() == Some(value);
        let mut entry_changes = Vec::new();
        let mut removed = Vec::new();
        let mut archives = Vec::new();
        for name in fsio::list_dir(&self.root)? {
            let archive = self.root.join(&name);
            if !name.starts_with(super::sessions::MOVED_PREFIX) || !fsio::inspect_subdir(&archive)?
            {
                continue;
            }
            let names = fsio::list_dir(&archive)?;
            for name in names.iter().filter(|n| is_moved_variant(n, ENTRIES_DIR)) {
                let dir = archive.join(name);
                if !fsio::inspect_subdir(&dir)? {
                    continue;
                }
                for tag in fsio::list_dir(&dir)? {
                    let tag_dir = dir.join(&tag);
                    if !fsio::inspect_subdir(&tag_dir)? {
                        continue;
                    }
                    for name in super::visible(fsio::list_dir(&tag_dir)?) {
                        let Some(stem) = name.strip_suffix(RECORD_EXT) else {
                            continue;
                        };
                        let path = tag_dir.join(&name);
                        if !fsio::inspect_file(&path)? {
                            continue;
                        }
                        let before = format::decode_entry(&path, &fsio::read(&path)?, stem, &tag)?;
                        let mut entry = before.clone();
                        if handoff_names(&entry, matches) {
                            removed.push(entry.id);
                            entry_changes.push((path, None));
                        } else {
                            detach_entry(&mut entry, matches, now);
                            if before != entry {
                                if entry.author.kind == crate::AuthorKind::Scribe {
                                    removed.push(entry.id.clone());
                                }
                                let bytes = format::encode_entry(&path, &entry)?;
                                entry_changes.push((path, Some(bytes)));
                            }
                        }
                    }
                }
            }
            archives.push((archive, names));
        }
        let mut changes = Vec::new();
        for (archive, names) in archives {
            for name in names {
                let path = archive.join(&name);
                if is_moved_variant(&name, PROPOSALS_FILE) && fsio::inspect_file(&path)? {
                    let before = format::decode_proposals(&path, &fsio::read(&path)?)?;
                    let mut after = before.clone();
                    detach_claims(&mut after, matches, &removed, now);
                    if before != after {
                        let bytes = format::encode_proposals(&path, &after)?;
                        changes.push((path, Some(bytes)));
                    }
                } else if is_moved_variant(&name, CONTROL_FILE) && fsio::inspect_file(&path)? {
                    let mut state = format::decode_control(&path, &fsio::read(&path)?)?;
                    let before = state.clone();
                    state.sessions.retain(|sid, _| !matches(sid.as_str()));
                    state.codex_cursors.retain(|sid, _| !matches(sid.as_str()));
                    for switch in state.sessions.values_mut() {
                        switch.pending_links.retain(|link| {
                            link.kind != crate::LinkType::Continues
                                || !matches(link.session.as_str())
                        });
                    }
                    if before != state {
                        let bytes = format::encode_control(&path, &state)?;
                        changes.push((path, Some(bytes)));
                    }
                } else if is_moved_variant(&name, MARKS_FILE) && fsio::inspect_file(&path)? {
                    let mut marks = format::decode_marks(&path, &fsio::read(&path)?)?;
                    let before = marks.len();
                    marks.retain(|mark| !matches(mark.session.as_str()));
                    if before != marks.len() {
                        let bytes = format::encode_marks(&path, &marks)?;
                        changes.push((path, Some(bytes)));
                    }
                } else if is_moved_variant(&name, PICKUPS_FILE) && fsio::inspect_file(&path)? {
                    let mut pickups = format::decode_pickups(&path, &fsio::read(&path)?)?;
                    let before = pickups.len();
                    pickups.retain(|pickup| !removed.contains(&pickup.entry));
                    if before != pickups.len() {
                        let bytes = format::encode_pickups(&path, &pickups)?;
                        changes.push((path, Some(bytes)));
                    }
                }
            }
        }
        // Keep the entries that identify archived claim quotes and pickups
        // until the documents that refer to them are clean.
        changes.extend(entry_changes);
        Ok(changes)
    }

    /// Drop `audit.jsonl*` lines matching `item`'s digests (plus extra
    /// pre-move proposal texts and the moved entry's archived text), rewrite
    /// or drop matching proposals in `proposals.json*`, and delete
    /// `item.entry`'s moved-aside entry file across every `pre-sqlite-*` dir.
    ///
    /// Runs under `locks/archive-forget.lock` (`_lock` witness) so concurrent
    /// forgets cannot restore one another's erased text (STD-03 R6). Cleaning
    /// `audit.jsonl*` before rewriting `proposals.json*` ensures a crash
    /// between the two cannot drop a pre-move-only proposal's text from audit
    /// scrubbing.
    pub(super) fn forget_moved_entry(
        &self,
        item: &docs::PendingArchiveForget,
        lock: &fsio::LockGuard,
    ) -> Result<()> {
        let id = &item.entry;
        let db_path = self.root.join(DB_FILE);
        let entry_file = format!("{id}{RECORD_EXT}");
        // One entry's words can sit in several `pre-sqlite-*` directories: an
        // interrupted move followed by a recovery on another date puts the
        // entries in one and `audit.jsonl` in another. So first read every
        // directory's proposal and entry texts, then scrub every audit log
        // with the union, and only then rewrite proposals and delete entry
        // files. A replay after an interruption reads whatever the earlier
        // run left, and an entry file is deleted only after every audit log
        // is clean, so the texts it holds are never lost before then.
        struct Moved {
            dir: PathBuf,
            names: Vec<String>,
            proposals: Vec<(PathBuf, Vec<Proposal>)>,
        }
        let mut dirs = Vec::new();
        let mut extra_texts = Vec::new();
        for moved_name in fsio::list_dir(&self.root)? {
            if !moved_name.starts_with(super::sessions::MOVED_PREFIX) {
                continue;
            }
            let moved = self.root.join(&moved_name);
            if !fsio::inspect_subdir(&moved)? {
                continue;
            }
            let names = fsio::list_dir(&moved)?;
            let mut proposals_files: Vec<(PathBuf, Vec<Proposal>)> = Vec::new();
            for name in names.iter().filter(|n| is_moved_variant(n, PROPOSALS_FILE)) {
                let path = moved.join(name);
                if fsio::inspect_file(&path)? {
                    let proposals = format::decode_proposals(&path, &fsio::read(&path)?)?;
                    #[cfg(test)]
                    notify_forget_moved(ForgetMovedStep::AfterProposalsRead);
                    collect_proposal_texts(id, &proposals, &mut extra_texts);
                    proposals_files.push((path, proposals));
                }
            }
            for name in names.iter().filter(|n| is_moved_variant(n, ENTRIES_DIR)) {
                let entries_dir = moved.join(name);
                if !fsio::inspect_subdir(&entries_dir)? {
                    continue;
                }
                let tag_dir = entries_dir.join(id.tag().as_str());
                if !fsio::inspect_subdir(&tag_dir)? {
                    continue;
                }
                let path = tag_dir.join(&entry_file);
                if fsio::inspect_file(&path)? {
                    let archived = format::decode_entry(
                        &path,
                        &fsio::read(&path)?,
                        &id.to_string(),
                        id.tag().as_str(),
                    )?;
                    extra_texts.extend(erased_texts(&archived, &[]));
                }
            }
            dirs.push(Moved {
                dir: moved,
                names,
                proposals: proposals_files,
            });
        }
        extra_texts.sort();
        extra_texts.dedup();
        let matcher = AuditMatcher::new(&item.salt, &item.digests, &extra_texts, &db_path)?;
        for moved in &dirs {
            for name in moved
                .names
                .iter()
                .filter(|n| is_moved_variant(n, AUDIT_FILE))
            {
                let path = moved.dir.join(name);
                if fsio::inspect_file(&path)? {
                    forget_moved_audit(&path, &matcher, lock)?;
                }
            }
        }
        for moved in &mut dirs {
            for (path, proposals) in std::mem::take(&mut moved.proposals) {
                rewrite_moved_proposals(id, &path, proposals, lock)?;
            }
        }
        for moved in &dirs {
            for name in moved
                .names
                .iter()
                .filter(|n| is_moved_variant(n, ENTRIES_DIR))
            {
                let entries_dir = moved.dir.join(name);
                if !fsio::inspect_subdir(&entries_dir)? {
                    continue;
                }
                let tag_dir = entries_dir.join(id.tag().as_str());
                if fsio::inspect_subdir(&tag_dir)? {
                    fsio::remove_private(&tag_dir.join(&entry_file))?;
                }
            }
        }
        Ok(())
    }
}

fn handoff_names(entry: &Entry, matches: impl Fn(&str) -> bool) -> bool {
    entry.tag.as_str() == crate::handoff::HANDOFF_TAG
        && entry
            .related
            .iter()
            .any(|r| r.kind() == crate::RelatedKind::Session && matches(r.target()))
}

/// Preserve independently recorded entries and mark the removed provenance.
fn detach_entry(entry: &mut Entry, matches: impl Fn(&str) -> bool, now: OffsetDateTime) {
    if entry
        .cite
        .as_ref()
        .is_some_and(|c| !c.is_deleted() && matches(c.session.as_str()))
    {
        if entry.author.kind == crate::AuthorKind::Scribe {
            let id = entry.id.clone();
            crate::consistency::forget(&id, &mut [], std::slice::from_mut(entry), now);
        }
        if let Some(cite) = &mut entry.cite {
            cite.mark_deleted();
        }
        if let crate::ContentFrom::Message { message, at, .. } = &mut entry.content_from {
            *message = String::from("deleted");
            *at = OffsetDateTime::UNIX_EPOCH;
        }
        entry.window = None;
    }
    let named = |value: &str| value.strip_prefix("session:").is_some_and(&matches);
    if entry.evidence.as_deref().is_some_and(named) {
        entry.evidence = Some(String::from("session:deleted"));
    }
    let source_reference =
        |r: &crate::Related| r.kind() == crate::RelatedKind::Session && matches(r.target());
    if entry.related.iter().any(source_reference) {
        entry.related.retain(|r| !source_reference(r));
        let marker = crate::Related::deleted_session();
        if !entry.related.contains(&marker) {
            entry.related.push(marker);
        }
    }
    for instruction in [&mut entry.instruction, &mut entry.removal_instruction]
        .into_iter()
        .flatten()
    {
        if matches(instruction.session.as_str()) {
            instruction.quote = None;
        }
    }
}

fn detach_claims(
    claims: &mut [Proposal],
    matches: impl Fn(&str) -> bool,
    removed: &[EntryId],
    now: OffsetDateTime,
) {
    for claim in claims {
        let source = claim
            .candidate
            .as_ref()
            .is_some_and(|c| !c.cite.is_deleted() && matches(c.cite.session.as_str()));
        let about = removed.contains(&claim.older)
            || claim.newer.as_ref().is_some_and(|id| removed.contains(id));
        if source || about {
            claim.reason.clear();
            for quote in &mut claim.quotes {
                quote.text.clear();
            }
            if let Some(candidate) = &mut claim.candidate {
                candidate.cite.mark_deleted();
                candidate.quote.clear();
                candidate.context = None;
                candidate.statement = None;
                candidate.subject = None;
                candidate.evidence = None;
                candidate.admitted = None;
                candidate.project = crate::Project::sanitized("deleted");
                candidate.provider = crate::Label::sanitized("deleted", "deleted");
                candidate.host = crate::Label::sanitized("deleted", "deleted");
            }
            if claim.state == crate::ProposalState::Open {
                claim.state = crate::ProposalState::Rejected;
                claim.closed_reason = Some(crate::ClosedReason::Withdrawn);
                claim.settled_at = Some(now);
                claim.settled_author = Some(crate::Author::consistency());
            }
        }
        let settlement_deleted = claim
            .settled_by
            .as_ref()
            .is_some_and(|c| !c.is_deleted() && matches(c.session.as_str()));
        if let Some(instruction) = &mut claim.instruction {
            if settlement_deleted || source {
                instruction.quote = None;
            }
        } else if settlement_deleted && let Some(cite) = &mut claim.settled_by {
            cite.mark_deleted();
        }
    }
}

pub(super) fn is_moved_variant(name: &str, base: &str) -> bool {
    name == base
        || name
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix('.'))
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
}

fn rewrite_moved_proposals(
    id: &EntryId,
    path: &Path,
    mut proposals: Vec<Proposal>,
    _lock: &fsio::LockGuard,
) -> Result<()> {
    let mut changed = crate::consistency::forget_proposals(id, &mut proposals);
    // An open pre-move candidate approved later cannot keep an empty quote
    // under `check_proposals`; drop it so `proposals.json` stays valid.
    let before_len = proposals.len();
    proposals.retain(|p| {
        !(p.kind == crate::ProposalKind::Candidate
            && p.state != crate::ProposalState::Approved
            && p.older == *id)
    });
    if proposals.len() != before_len {
        changed = true;
    }
    if changed {
        let encoded = format::encode_proposals(path, &proposals)?;
        fsio::write_atomic(path, &encoded)?;
    }
    Ok(())
}

fn forget_moved_audit(
    path: &Path,
    matcher: &AuditMatcher<'_>,
    _lock: &fsio::LockGuard,
) -> Result<()> {
    if matcher.is_empty() {
        return Ok(());
    }
    let bytes = fsio::read(path)?;
    #[cfg(test)]
    notify_forget_moved(ForgetMovedStep::AfterAuditRead);
    let mut kept = Vec::with_capacity(bytes.len());
    let mut dropped = false;
    for raw_with_nl in bytes.split_inclusive(|&b| b == b'\n') {
        let raw = raw_with_nl.strip_suffix(b"\n").unwrap_or(raw_with_nl);
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        if !raw.iter().all(u8::is_ascii_whitespace) && audit_line_carries_text(raw, matcher) {
            dropped = true;
            continue;
        }
        kept.extend_from_slice(raw_with_nl);
    }
    if dropped {
        fsio::write_atomic(path, &kept)?;
    }
    Ok(())
}

/// Domain separation prefix for salted SHA-256 digests stored in
/// `pending_archive_forgets`.
const ARCHIVE_FORGET_DOMAIN: &[u8] = b"scribe-archive-forget-v1\0";

fn forget_salt(id: &EntryId, root: &Path) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seed = format!(
        "archive-forget|{id}|{}|{}|{}|{}",
        std::process::id(),
        OffsetDateTime::now_utc().unix_timestamp_nanos(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        root.display(),
    );
    crate::capture::prompt_sha256(&seed)
}

fn salted_prefix(salt: &str) -> Sha256 {
    let mut hasher = Sha256::new();
    hasher.update(ARCHIVE_FORGET_DOMAIN);
    hasher.update(salt.as_bytes());
    hasher.update(b"\0");
    hasher
}

fn digest_texts(salt: &str, texts: &[String]) -> Vec<docs::TextDigest> {
    let prefix = salted_prefix(salt);
    let mut out: Vec<docs::TextDigest> = texts
        .iter()
        .filter(|t| !t.is_empty())
        .map(|text| {
            let mut hasher = prefix.clone();
            hasher.update(text.as_bytes());
            let digest: [u8; 32] = hasher.finalize().into();
            docs::TextDigest {
                len: text.len(),
                sha256: encode_hex_32(&digest),
            }
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn encode_hex_32(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn decode_hex_32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let nibble = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
        let hi = nibble(pair[0])?;
        let lo = nibble(pair[1])?;
        *slot = (hi << 4) | lo;
    }
    Some(out)
}

/// Matches free-text values in archived `audit.jsonl*` lines against durable
/// salted SHA-256 digests (without cleartext) plus any extra proposal texts
/// freshly read from `proposals.json*` before it is rewritten.
struct AuditMatcher<'a> {
    prefix: Sha256,
    digests_by_len: BTreeMap<usize, Vec<[u8; 32]>>,
    extra_texts: &'a [String],
}

impl<'a> AuditMatcher<'a> {
    fn new(
        salt: &str,
        digests: &[docs::TextDigest],
        extra_texts: &'a [String],
        db_path: &Path,
    ) -> Result<Self> {
        let mut digests_by_len: BTreeMap<usize, Vec<[u8; 32]>> = BTreeMap::new();
        for d in digests {
            let bytes = decode_hex_32(&d.sha256).ok_or_else(|| {
                Error::corrupt(db_path, "pending archive forget digest is not 64 hex chars")
            })?;
            digests_by_len.entry(d.len).or_default().push(bytes);
        }
        for group in digests_by_len.values_mut() {
            group.sort();
            group.dedup();
        }
        Ok(Self {
            prefix: salted_prefix(salt),
            digests_by_len,
            extra_texts,
        })
    }

    fn is_empty(&self) -> bool {
        self.digests_by_len.is_empty() && self.extra_texts.is_empty()
    }

    fn matches_str(&self, haystack: &str) -> bool {
        if self
            .extra_texts
            .iter()
            .any(|t| haystack.contains(t.as_str()))
        {
            return true;
        }
        for (&len, targets) in &self.digests_by_len {
            if len == 0 || haystack.len() < len {
                continue;
            }
            for (start, _) in haystack.char_indices() {
                let Some(end) = start.checked_add(len) else {
                    break;
                };
                if end > haystack.len() {
                    break;
                }
                if !haystack.is_char_boundary(end) {
                    continue;
                }
                let mut hasher = self.prefix.clone();
                hasher.update(&haystack.as_bytes()[start..end]);
                let digest: [u8; 32] = hasher.finalize().into();
                if targets.binary_search(&digest).is_ok() {
                    return true;
                }
            }
        }
        false
    }
}

/// The keys of an audit line's structural fields (`AuditEvent`): timestamps,
/// command paths, statuses, record ids, codes, agent names, counts and the
/// `issue` record's ids, config trigger and hash. They never hold free text,
/// so no entry's words are matched under them, however common the word: an
/// entry whose body is `success`, an agent's name or `hook stop` leaves every
/// unrelated line alone.
const AUDIT_STANDARD_KEYS: &[&str] = &[
    "format",
    "at",
    "command",
    "status",
    "target",
    "error_code",
    "author",
    "instruction_session",
    "instruction_message",
    "issue",
    "duration_ms",
];

fn audit_line_carries_text(raw: &[u8], matcher: &AuditMatcher<'_>) -> bool {
    match serde_json::from_slice::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Object(map)) => map.iter().any(|(k, v)| {
            !AUDIT_STANDARD_KEYS.contains(&k.as_str()) && value_carries_text(v, matcher)
        }),
        Ok(other) => value_carries_text(&other, matcher),
        // A torn or hand-edited line has no structure to read, and the reader
        // skips it, so any copy of the words in it is erased.
        Err(_) => {
            let line = String::from_utf8_lossy(raw);
            matcher.matches_str(&line)
        }
    }
}

/// Whether a free-text value (a key outside `AUDIT_STANDARD_KEYS`, or any
/// unrecognised line shape) contains one of the erased texts.
fn value_carries_text(value: &serde_json::Value, matcher: &AuditMatcher<'_>) -> bool {
    match value {
        serde_json::Value::String(s) => matcher.matches_str(s),
        serde_json::Value::Array(items) => items.iter().any(|v| value_carries_text(v, matcher)),
        serde_json::Value::Object(map) => map.values().any(|v| value_carries_text(v, matcher)),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            false
        }
    }
}

fn push_non_empty(out: &mut Vec<String>, text: &str) {
    if !text.trim().is_empty() {
        out.push(text.to_owned());
    }
}

fn collect_proposal_texts(id: &EntryId, proposals: &[Proposal], out: &mut Vec<String>) {
    for p in proposals {
        if p.older == *id || p.newer.as_ref() == Some(id) {
            push_non_empty(out, &p.reason);
            if let Some(c) = &p.candidate {
                push_non_empty(out, &c.quote);
                for opt in [
                    c.context.as_deref(),
                    c.statement.as_deref(),
                    c.subject.as_deref(),
                    c.evidence.as_deref(),
                    c.admitted.as_ref().map(|a| a.matched.as_str()),
                ]
                .into_iter()
                .flatten()
                {
                    push_non_empty(out, opt);
                }
            }
            if let Some(quote) = p.instruction.as_ref().and_then(|i| i.quote.as_deref()) {
                push_non_empty(out, quote);
            }
        }
        for q in p.quotes.iter().filter(|q| q.entry == *id) {
            push_non_empty(out, &q.text);
        }
    }
}

fn erased_texts(entry: &Entry, proposals: &[Proposal]) -> Vec<String> {
    let mut out = Vec::new();
    push_non_empty(&mut out, &entry.content);
    push_non_empty(&mut out, &entry.said);
    for opt in [
        entry.quote.as_deref(),
        entry.evidence.as_deref(),
        entry.retirement_reason.as_deref(),
        entry.instruction.as_ref().and_then(|i| i.quote.as_deref()),
        entry
            .removal_instruction
            .as_ref()
            .and_then(|i| i.quote.as_deref()),
        entry.outcome.as_ref().and_then(|o| o.reason.as_deref()),
    ]
    .into_iter()
    .flatten()
    {
        push_non_empty(&mut out, opt);
    }
    if let Some(window) = &entry.window {
        for turn in &window.turns {
            push_non_empty(&mut out, &turn.text);
        }
    }
    collect_proposal_texts(&entry.id, proposals, &mut out);
    out.sort();
    out.dedup();
    out
}

fn is_empty(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::Array(a) => a.is_empty(),
        serde_json::Value::Object(o) => o.is_empty(),
        serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => false,
    }
}
