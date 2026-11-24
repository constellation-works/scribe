//! Entries, claims, store tags, marks, pickups and the audit log in
//! `scribe.db` (docs/design/scribe/2_design.md §15.7, phase 7), where they
//! are read from, the first write's import of the file store's archives,
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
//!   tables (`archive`), and drains the spooled audit lines.
//! - **Stamps.** `control.json`, `proposals.json`, `tags.json`, `marks.json`
//!   and `pickups.json` are empty documents at the current format, so an
//!   older build refuses the store.

use super::audit::{self, AuditEvent};
use super::db::{self, Conn as Connection, DB_FILE, docs};
use super::write::Move;
use super::{
    AUDIT_FILE, CONTROL_FILE, ENTRIES_DIR, MARKS_FILE, PICKUPS_FILE, PROPOSALS_FILE, RECORD_EXT,
    SESSIONS_DIR, Skipped, Store, codec, fsio,
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
use std::path::Path;
use time::OffsetDateTime;

/// The store tags' stamp file.
pub(super) const TAGS_FILE: &str = "tags.json";

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
            self.refuse_file_store()?;
            return Ok((Vec::new(), Vec::new()));
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
    pub(crate) fn entry(&self, id: &EntryId) -> Result<Entry> {
        let path = self.root.join(DB_FILE);
        if let Some(rows) = self.read_db(docs::entries)? {
            for row in rows {
                match row {
                    Ok(entry) if &entry.id == id => return Ok(entry),
                    Err(bad) if bad.id.as_ref() == Some(id) => return Err(bad.error),
                    Ok(_) | Err(_) => {}
                }
            }
        } else {
            self.refuse_file_store()?;
        }
        Err(Error::EntryNotFound {
            id: id.clone(),
            path,
        })
    }

    /// Every proposal, by id; none when there are none.
    pub(crate) fn proposals(&self) -> Result<Vec<Proposal>> {
        let mut proposals = self.read_or_empty(docs::claims)?;
        proposals.sort_by_key(|p| p.id);
        Ok(proposals)
    }

    /// The store tags (`scribe tag add`), without writing.
    pub(crate) fn store_tags(&self) -> Result<Vec<StoreTag>> {
        let mut tags = self.read_or_empty(docs::tags)?;
        tags.retain(|tag| !crate::entry::is_decision_or_item(tag.name.as_str()));
        Ok(tags)
    }

    /// Every pickup, oldest first; none for a missing store. Reads only.
    pub(crate) fn pickups(&self) -> Result<Vec<Pickup>> {
        self.read_or_empty(docs::pickups)
    }

    /// Every marker outcome, in order.
    pub(super) fn all_marks(&self) -> Result<Vec<MarkRecord>> {
        self.read_or_empty(docs::marks)
    }

    /// `f` on the document tables, or nothing for a store with no
    /// database yet; a file store is refused.
    fn read_or_empty<T>(&self, f: impl Fn(&Connection, &Path) -> Result<Vec<T>>) -> Result<Vec<T>> {
        match self.read_db(f)? {
            Some(found) => Ok(found),
            None => {
                self.refuse_file_store()?;
                Ok(Vec::new())
            }
        }
    }

    /// The audit log's events, oldest first, the spooled ones last, and how
    /// many lines did not parse. Reads only: no lock, no write; a store with
    /// none has none.
    ///
    /// # Errors
    ///
    /// An unsafe file (a link, loose modes) as for any store read, or a
    /// line of a newer audit format (`store_too_new`).
    pub(crate) fn audit_events(&self) -> Result<(Vec<AuditEvent>, usize)> {
        let (lines, torn) = self.audit_lines()?;
        Ok((lines.into_iter().map(|(_, event)| event).collect(), torn))
    }

    /// Audit lines with their original formats, in the same order as `audit_events`.
    fn audit_lines(&self) -> Result<(Vec<(u32, AuditEvent)>, usize)> {
        if !self.root_exists()? {
            return Ok((Vec::new(), 0));
        }
        let (mut lines, drained) = match self.read_db(|conn, path| {
            let drained = docs::spooled_names(conn, path)?;
            Ok((docs::audit(conn, path)?, drained))
        })? {
            Some(found) => found,
            None => {
                self.refuse_file_store()?;
                (Vec::new(), Vec::new())
            }
        };
        let (spooled, torn) = self.spooled_audit()?;
        lines.extend(
            spooled
                .into_iter()
                .filter(|(name, _, _)| !drained.contains(name))
                .map(|(_, format, event)| (format, event)),
        );
        Ok((lines, torn))
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

    /// At the start of a write's transaction: record a new store's kinds as
    /// in the database, import the file store's archives (and any files an
    /// interrupted move left in the root), and drain the spooled audit
    /// lines. A file store is refused: its records never moved. `store
    /// migrate` (`mode`) is refused first when there is nothing to import
    /// or the import is unsafe.
    pub(super) fn import_at_write(&self, mode: Move, now: OffsetDateTime) -> Result<Imported> {
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
            self.refuse_migrate(&conn, &leftovers)?;
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
    pub(super) fn file_store_items(&self) -> Result<Vec<String>> {
        let mut items = Vec::new();
        if !self.root_exists()? {
            return Ok(items);
        }
        for dir in [SESSIONS_DIR, ENTRIES_DIR, super::ISSUES_DIR] {
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
        let memory = self.root.join(super::MEMORY_DIR);
        if fsio::inspect_subdir(&memory)? {
            for name in fsio::list_dir(&memory)? {
                if name.starts_with(LEGACY_MEMORY_PREFIX)
                    && name.ends_with(RECORD_EXT)
                    && fsio::inspect_file(&memory.join(&name))?
                {
                    items.push(format!("{}/{name}", super::MEMORY_DIR));
                }
            }
        }
        Ok(items)
    }

    /// Refuse a file store whose records never moved into `scribe.db`.
    pub(super) fn refuse_file_store(&self) -> Result<()> {
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
    pub(super) fn export_documents(
        &self,
        target: &Path,
        write: &mut dyn FnMut(String, &[u8]) -> Result<()>,
        out: &mut super::ExportCounts,
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

    /// Erase `entry` from the stored messages it cites and from the archive
    /// tables, in the forget's own write (`proposals`: the claims before
    /// the forget).
    pub(super) fn forget_entry_residue(
        &self,
        entry: &Entry,
        proposals: &[Proposal],
        _now: OffsetDateTime,
    ) -> Result<()> {
        // The cited turns' stored messages go first: they hold the words
        // the forget removes, and deleting them again is a no-op.
        self.forget_cited_messages(entry, proposals)?;
        self.with_conn(|conn, path| super::archive::scrub_entry(conn, path, entry, proposals))
            .map(drop)
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
        if let Some(decision) = &mut entry.decision {
            entry.quote = None;
            decision.reason_quote = None;
        }
        let marker = crate::Related::deleted_session();
        if !entry.related.contains(&marker) {
            entry.related.push(marker);
        }
    }
    if let Some(item) = &mut entry.item
        && item
            .answer
            .as_ref()
            .is_some_and(|a| matches(a.session.as_str()))
    {
        // The answer is the user's verbatim words from the session; erase
        // them with the id. An unsettled item is open again.
        item.answer = None;
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

/// Domain separation prefix for salted SHA-256 digests stored in
/// `pending_archive_forgets` before phase 8.
const ARCHIVE_FORGET_DOMAIN: &[u8] = b"scribe-archive-forget-v1\0";

fn salted_prefix(salt: &str) -> Sha256 {
    let mut hasher = Sha256::new();
    hasher.update(ARCHIVE_FORGET_DOMAIN);
    hasher.update(salt.as_bytes());
    hasher.update(b"\0");
    hasher
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

/// Matches text against the salted SHA-256 digests (`{len, sha256}`) a
/// forget recorded, before phase 8, for archives it had not cleaned yet:
/// the archive import replays them on the imported records.
pub(super) struct DigestMatcher {
    prefix: Sha256,
    digests_by_len: BTreeMap<usize, Vec<[u8; 32]>>,
}

impl DigestMatcher {
    pub(super) fn new(salt: &str, digests: &[docs::TextDigest], db_path: &Path) -> Result<Self> {
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
        })
    }

    pub(super) fn matches_str(&self, haystack: &str) -> bool {
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

pub(super) fn erased_texts(entry: &Entry, proposals: &[Proposal]) -> Vec<String> {
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
    if entry.decision.is_some() || entry.item.is_some() {
        let mut record = entry.clone();
        out.extend(
            super::redact::entry_fields(&mut record)
                .into_iter()
                .filter_map(|(name, text)| {
                    (name.starts_with("entry.decision.") || name.starts_with("entry.item."))
                        .then(|| text.clone())
                }),
        );
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
