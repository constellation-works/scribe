//! The documents as a whole in `scribe.db`: entries, claims, store tags,
//! marks, pickups and the audit log (docs/design/scribe/2_design.md §15.7,
//! phase 7), whether they moved in and the reads over them, the first write's import of the file store's archives,
//! and their part of `scribe export`.
//!
//! - **Reads.** Every read comes from the database. A root that still holds
//!   a file store (records in `sessions/`, `entries/`, the JSON documents or
//!   `audit.jsonl`, and no `scribe.db` they moved into) is refused: this
//!   build has no file decoder (phase 8). Reads take no lock and write
//!   nothing (STD-01 R31).
//! - **The first write.** [`Store::import_at_write`] runs at the start of the
//!   first write's transaction: it records a new store's kinds as moved,
//!   imports the `pre-sqlite-*` and `pre-issue-*` archives into the archive
//!   tables (`db/import`), and drains the spooled audit lines.
//! - **Stamps.** `control.json`, `proposals.json`, `tags.json`, `marks.json`
//!   and `pickups.json` are empty documents at the current format, so an
//!   older build refuses the store.

use crate::db::import::is_empty;
use crate::db::write::Move;
use crate::db::{self, Conn as Connection, DB_FILE, codec, docs};
use crate::error::{Error, Result};
use crate::repository::audit;
use crate::repository::tags::TAGS_FILE;
use crate::session::ControlState;
use crate::{
    AUDIT_FILE, CONTROL_FILE, ENTRIES_DIR, MARKS_FILE, PICKUPS_FILE, PROPOSALS_FILE, RECORD_EXT,
    SESSIONS_DIR, Store, fsio,
};
use std::path::Path;
use time::OffsetDateTime;

/// The legacy memory items' directory entries (`memory/m-<n>.md`).
const LEGACY_MEMORY_PREFIX: &str = "m-";

/// What `scribe store migrate`, or a write's own import, did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// Session records imported (always 0 since phase 8: only archives are
    /// imported).
    pub sessions: usize,
    /// Switches imported (always 0 since phase 8).
    pub switches: usize,
    /// Codex cursors imported (always 0 since phase 8).
    pub cursors: usize,
    /// Rebuild checkpoints imported (always 0 since phase 8).
    pub checkpoints: usize,
    /// Entries imported (always 0 since phase 8).
    pub entries: usize,
    /// Claims imported (always 0 since phase 8).
    pub claims: usize,
    /// Store tags imported (always 0 since phase 8).
    pub tags: usize,
    /// Annotation marks imported (always 0 since phase 8).
    pub marks: usize,
    /// Pickups imported (always 0 since phase 8).
    pub pickups: usize,
    /// Spooled audit lines drained into the table.
    pub audit_lines: usize,
    /// Audit lines that did not parse (always 0 since phase 8).
    pub audit_torn: usize,
    /// Where files went (always `None` since phase 8: archives are
    /// imported, then removed).
    pub moved_to: Option<String>,
    /// `pre-sqlite-*` and `pre-issue-*` directories imported into the
    /// archive tables.
    pub archives: usize,
    /// Archived records imported.
    pub archived: usize,
}

impl Store {
    /// Whether entries, claims, tags, marks, pickups and the audit log
    /// live in `scribe.db`. Reads only.
    pub(crate) fn documents_moved(&self) -> Result<bool> {
        if !self.root_exists()? {
            return Ok(false);
        }
        Ok(docs::read(&self.root, |_, _| Ok(()))?.is_some())
    }

    /// Run `f` on the document tables when the documents live in the
    /// database (after checking the format stamp), else `None`.
    pub(crate) fn read_db<T>(
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

    /// `f` on the document tables, or nothing for a store with no
    /// database yet; a file store is refused.
    pub(crate) fn read_or_empty<T>(
        &self,
        f: impl Fn(&Connection, &Path) -> Result<Vec<T>>,
    ) -> Result<Vec<T>> {
        match self.read_db(f)? {
            Some(found) => Ok(found),
            None => {
                self.refuse_file_store()?;
                Ok(Vec::new())
            }
        }
    }

    /// At the start of a write's transaction: record a new store's kinds as
    /// in the database, import the file store's archives (and any files an
    /// interrupted move left in the root), and drain the spooled audit
    /// lines. A file store is refused: its records never moved. `store
    /// migrate` (`mode`) is refused first when there is nothing to import
    /// or the import is unsafe.
    pub(crate) fn import_at_write(&self, mode: Move, now: OffsetDateTime) -> Result<Imported> {
        let conn = db::open_writer(&self.root, self.lock_wait)?;
        let path = self.root.join(DB_FILE);
        let fail = |e| db::db_error("move documents into", &path, e);
        let sessions_moved = db::sessions::moved(&conn).map_err(fail)?;
        let documents_moved = docs::moved(&conn).map_err(fail)?;
        let leftovers = self.file_store_items()?;
        if !(leftovers.is_empty() || sessions_moved && documents_moved) {
            return Err(self.file_store_error());
        }
        if mode == Move::ByHand {
            self.refuse_import(&conn, &leftovers)?;
        }
        if !sessions_moved {
            db::sessions::mark_moved(&conn, now, None).map_err(fail)?;
        }
        if !documents_moved {
            docs::mark_moved(&conn, now, |_| None).map_err(fail)?;
            self.after(|a| a.stamp = true);
        }
        let mut moved = Imported::default();
        let archived = self.import_archives(&conn, now)?;
        let left = self.import_leftovers(&conn, &leftovers, now)?;
        moved.archives = archived.archives + left.archives;
        moved.archived = archived.records + left.records;
        if !leftovers.is_empty() {
            self.after(|a| a.stamp = true);
        }
        moved.audit_lines += self.drain_spool(&conn, &path)?;
        Ok(moved)
    }

    /// Items in the root that only a file store holds: `sessions/`,
    /// `entries/`, `issues/`, `audit.jsonl`, a JSON document with records
    /// in it (an empty one is a stamp), the root's Codex lock files and
    /// legacy `memory/m-<n>.md` items. Read by name and as plain JSON, never
    /// decoded as records.
    pub(crate) fn file_store_items(&self) -> Result<Vec<String>> {
        let mut items = Vec::new();
        if !self.root_exists()? {
            return Ok(items);
        }
        for dir in [SESSIONS_DIR, ENTRIES_DIR, crate::ISSUES_DIR] {
            if fsio::inspect_subdir(&self.root.join(dir))? {
                items.push(dir.to_owned());
            }
        }
        if fsio::inspect_file(&self.root.join(AUDIT_FILE))? {
            items.push(AUDIT_FILE.to_owned());
        }
        for name in [
            CONTROL_FILE,
            PROPOSALS_FILE,
            TAGS_FILE,
            MARKS_FILE,
            PICKUPS_FILE,
        ] {
            if self.document_has_content(name)? {
                items.push(name.to_owned());
            }
        }
        items.extend(
            fsio::list_dir(&self.root)?
                .into_iter()
                .filter(|n| n.starts_with(".codex-") && n.ends_with(".lock")),
        );
        let memory = self.root.join(crate::MEMORY_DIR);
        if fsio::inspect_subdir(&memory)? {
            for name in fsio::list_dir(&memory)? {
                if name.starts_with(LEGACY_MEMORY_PREFIX)
                    && name.ends_with(RECORD_EXT)
                    && fsio::inspect_file(&memory.join(&name))?
                {
                    items.push(format!("{}/{name}", crate::MEMORY_DIR));
                }
            }
        }
        Ok(items)
    }

    /// Refuse a file store whose records never moved into `scribe.db`.
    pub(crate) fn refuse_file_store(&self) -> Result<()> {
        if self.file_store_items()?.is_empty() {
            return Ok(());
        }
        let sessions = db::sessions::read_moved(&self.root)?;
        if sessions && self.documents_moved()? {
            // Files an interrupted move left beside the database; the next
            // write archives them.
            return Ok(());
        }
        Err(self.file_store_error())
    }

    fn file_store_error(&self) -> Error {
        Error::corrupt(
            &self.root,
            "this is a file store whose records never moved into scribe.db, and this build no longer reads one; run `scribe store migrate` with an earlier scribe build, then use this one",
        )
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

    /// Rewrite the format stamps, or note it for after the write under way.
    pub(crate) fn stamp_format(&self) -> Result<()> {
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
            &codec::encode_control(&control, &ControlState::default())?,
        )?;
        let proposals = self.root.join(PROPOSALS_FILE);
        fsio::write_atomic(&proposals, &codec::encode_proposals(&proposals, &[])?)?;
        let tags = self.root.join(TAGS_FILE);
        fsio::write_atomic(&tags, &codec::encode_tags(&tags, &[])?)?;
        let marks = self.root.join(MARKS_FILE);
        fsio::write_atomic(&marks, &codec::encode_marks(&marks, &[])?)?;
        let pickups = self.root.join(PICKUPS_FILE);
        fsio::write_atomic(&pickups, &codec::encode_pickups(&pickups, &[])?)
    }

    /// `scribe export --as markdown` of the documents: one file per entry,
    /// `proposals.json`, `tags.json`, `marks.json`, `pickups.json` and
    /// `audit.jsonl`, at the current format: a copy to read and keep, which
    /// no build reads back as a store.
    pub(crate) fn export_documents(
        &self,
        target: &Path,
        write: &mut dyn FnMut(String, &[u8]) -> Result<()>,
        out: &mut crate::ExportCounts,
    ) -> Result<()> {
        let (mut entries, skipped) = self.load_entries()?;
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        for entry in &entries {
            let rel = format!(
                "{ENTRIES_DIR}/{}/{}{RECORD_EXT}",
                entry.tag.as_str(),
                entry.id
            );
            let bytes = codec::encode_entry(&target.join(&rel), entry)?;
            write(rel, &bytes)?;
        }
        out.entries = entries.len();
        out.skipped.extend(skipped.into_iter().map(|s| s.path));
        let proposals = self.proposals()?;
        let path = target.join(PROPOSALS_FILE);
        write(
            PROPOSALS_FILE.to_owned(),
            &codec::encode_proposals(&path, &proposals)?,
        )?;
        out.claims = proposals.len();
        let path = target.join(TAGS_FILE);
        write(
            TAGS_FILE.to_owned(),
            &codec::encode_tags(&path, &self.store_tags()?)?,
        )?;
        let path = target.join(MARKS_FILE);
        write(
            MARKS_FILE.to_owned(),
            &codec::encode_marks(&path, &self.all_marks()?)?,
        )?;
        let path = target.join(PICKUPS_FILE);
        write(
            PICKUPS_FILE.to_owned(),
            &codec::encode_pickups(&path, &self.pickups()?)?,
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
}
