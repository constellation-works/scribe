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
use crate::session::ControlState;
use crate::tag::StoreTag;
use std::path::Path;
use time::OffsetDateTime;

/// The store tags' file, before the move.
pub(super) const TAGS_FILE: &str = "tags.json";

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
    fn read_db<T>(&self, f: impl Fn(&Connection, &Path) -> Result<T>) -> Result<Option<T>> {
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
        if !self.root_exists()? {
            return Ok((Vec::new(), 0));
        }
        let (mut events, mut torn, drained) = match self.read_db(|conn, path| {
            let drained = docs::spooled_names(conn, path)?;
            Ok((docs::audit(conn, path)?, drained))
        })? {
            Some((events, drained)) => (events, 0, drained),
            None => {
                let (events, torn) = self.audit_file()?;
                (
                    events.into_iter().map(|(_, e)| e).collect(),
                    torn,
                    Vec::new(),
                )
            }
        };
        let (spooled, spool_torn) = self.spooled_audit()?;
        torn += spool_torn;
        events.extend(
            spooled
                .into_iter()
                .filter(|(name, _, _)| !drained.contains(name))
                .map(|(_, _, e)| e),
        );
        Ok((events, torn))
    }

    /// `audit.jsonl`'s lines with their formats, and how many did not parse.
    fn audit_file(&self) -> Result<(Vec<(u32, AuditEvent)>, usize)> {
        let path = self.root.join(AUDIT_FILE);
        if !fsio::inspect_file(&path)? {
            return Ok((Vec::new(), 0));
        }
        audit::decode_formatted(&path, &fsio::read(&path)?)
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
            docs::mark_moved(&conn, now, None).map_err(fail)?;
            self.after(|a| a.aside = true);
        } else if self.audit_file_present()? {
            // A line an older build appended after the move: kept.
            let (lines, torn) = self.audit_file()?;
            for (format, event) in &lines {
                docs::put_audit(&conn, &path, *format, event, None, fail)?;
            }
            moved.audit_lines += lines.len();
            moved.audit_torn += torn;
            self.after(|a| a.aside = true);
        }
        if self.has_files_to_move_aside()? {
            self.after(|a| a.aside = true);
        }
        let spooled = self.spooled_audit()?.0.len();
        self.drain_spool(&conn, &path)?;
        moved.audit_lines += spooled;
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
        let (lines, torn) = self.audit_file()?;
        if torn > 0 {
            tracing::warn!(
                torn,
                "audit.jsonl lines that do not parse were not moved into scribe.db"
            );
        }
        for (format, event) in &lines {
            docs::put_audit(conn, path, *format, event, None, fail)?;
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

    /// Move every file that now lives in the database to
    /// `pre-sqlite-<date>/` (sessions, entries, the JSON documents, the
    /// audit log, the root's Codex lock files), then write the stamps.
    /// Moves nothing that is not there, so it finishes an interrupted move.
    pub(super) fn move_files_aside(&self, now: OffsetDateTime) -> Result<Option<String>> {
        let mut moves: Vec<String> = Vec::new();
        for dir in [SESSIONS_DIR, ENTRIES_DIR] {
            if fsio::inspect_subdir(&self.root.join(dir))? {
                moves.push(dir.to_owned());
            }
        }
        if self.control_has_content()? {
            moves.push(CONTROL_FILE.to_owned());
        }
        for name in [PROPOSALS_FILE, TAGS_FILE, MARKS_FILE, PICKUPS_FILE] {
            if self.document_has_content(name)? {
                moves.push(name.to_owned());
            }
        }
        if self.audit_file_present()? {
            moves.push(AUDIT_FILE.to_owned());
        }
        moves.extend(
            fsio::list_dir(&self.root)?
                .into_iter()
                .filter(|n| n.starts_with(".codex-") && n.ends_with(".lock")),
        );
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
        let (events, _) = self.audit_events()?;
        let mut log = Vec::new();
        for event in &events {
            log.extend(audit::encode_line(event).map_err(|e| {
                Error::corrupt(
                    target.join(AUDIT_FILE),
                    format!("an audit line does not encode: {e}"),
                )
            })?);
        }
        write(AUDIT_FILE.to_owned(), &log)?;
        out.audit_lines = events.len();
        Ok(())
    }

    /// Delete the stored messages cited by `entry` and schedule cleanup of
    /// its moved-aside `pre-sqlite-*` files once the transaction commits.
    pub(super) fn forget_entry_residue(&self, entry: &Entry, proposals: &[Proposal]) -> Result<()> {
        // The cited turns' stored messages go first: they hold the words
        // the forget removes, and deleting them again is a no-op.
        self.forget_cited_messages(entry, proposals)?;
        let texts = erased_texts(entry, proposals);
        let id = entry.id.clone();
        let queued = self.after(|after| {
            after.forget_entries.push(super::write::ForgetEntryCleanup {
                id: id.clone(),
                texts: texts.clone(),
            });
        });
        if !queued {
            self.forget_moved_entry(&id, &texts)?;
        }
        Ok(())
    }

    /// Delete `id`'s moved-aside entry file, rewrite or drop matching
    /// proposals in `proposals.json*`, and drop `audit.jsonl*` lines carrying
    /// `texts` across every `pre-sqlite-*` directory.
    pub(super) fn forget_moved_entry(&self, id: &EntryId, texts: &[String]) -> Result<()> {
        if !self.root_exists()? {
            return Ok(());
        }
        let entry_file = format!("{id}{RECORD_EXT}");
        for moved_name in fsio::list_dir(&self.root)? {
            if !moved_name.starts_with(super::sessions::MOVED_PREFIX) {
                continue;
            }
            let moved = self.root.join(&moved_name);
            if !fsio::inspect_subdir(&moved)? {
                continue;
            }
            let mut all_texts = texts.to_vec();
            let names = fsio::list_dir(&moved)?;
            for name in names.iter().filter(|n| is_moved_variant(n, ENTRIES_DIR)) {
                let entries_dir = moved.join(name);
                if !fsio::inspect_subdir(&entries_dir)? {
                    continue;
                }
                let tag_dir = entries_dir.join(id.tag().as_str());
                if fsio::inspect_subdir(&tag_dir)? {
                    fsio::remove_private(&tag_dir.join(&entry_file))?;
                }
            }
            for name in names.iter().filter(|n| is_moved_variant(n, PROPOSALS_FILE)) {
                let path = moved.join(name);
                if fsio::inspect_file(&path)? {
                    forget_moved_proposals(id, &path, &mut all_texts)?;
                }
            }
            all_texts.sort();
            all_texts.dedup();
            for name in names.iter().filter(|n| is_moved_variant(n, AUDIT_FILE)) {
                let path = moved.join(name);
                if fsio::inspect_file(&path)? {
                    forget_moved_audit(&path, &all_texts)?;
                }
            }
        }
        Ok(())
    }
}

fn is_moved_variant(name: &str, base: &str) -> bool {
    name == base
        || name
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix('.'))
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
}

fn forget_moved_proposals(id: &EntryId, path: &Path, texts: &mut Vec<String>) -> Result<()> {
    let mut proposals = format::decode_proposals(path, &fsio::read(path)?)?;
    collect_proposal_texts(id, &proposals, texts);
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

fn forget_moved_audit(path: &Path, texts: &[String]) -> Result<()> {
    if texts.is_empty() {
        return Ok(());
    }
    let bytes = fsio::read(path)?;
    let mut kept = Vec::with_capacity(bytes.len());
    let mut dropped = false;
    for raw_with_nl in bytes.split_inclusive(|&b| b == b'\n') {
        let raw = raw_with_nl.strip_suffix(b"\n").unwrap_or(raw_with_nl);
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        if !raw.iter().all(u8::is_ascii_whitespace) && audit_line_carries_text(raw, texts) {
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

fn audit_line_carries_text(raw: &[u8], texts: &[String]) -> bool {
    match serde_json::from_slice::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Object(map)) => map.iter().any(|(k, v)| {
            !AUDIT_STANDARD_KEYS.contains(&k.as_str()) && value_carries_text(v, texts)
        }),
        Ok(other) => value_carries_text(&other, texts),
        // A torn or hand-edited line has no structure to read, and the reader
        // skips it, so any copy of the words in it is erased.
        Err(_) => {
            let line = String::from_utf8_lossy(raw);
            texts.iter().any(|t| line.contains(t.as_str()))
        }
    }
}

/// Whether a free-text value (a key outside `AUDIT_STANDARD_KEYS`, or any
/// unrecognised line shape) contains one of `texts`.
fn value_carries_text(value: &serde_json::Value, texts: &[String]) -> bool {
    match value {
        serde_json::Value::String(s) => texts.iter().any(|t| s.contains(t.as_str())),
        serde_json::Value::Array(items) => items.iter().any(|v| value_carries_text(v, texts)),
        serde_json::Value::Object(map) => map.values().any(|v| value_carries_text(v, texts)),
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
