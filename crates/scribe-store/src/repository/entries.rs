//! The entry repository: notes, findings, handoffs, decisions and open
//! items in `scribe.db`, their listing and lookup, and a forget's erasure
//! of an entry or of a session's handoffs.

use super::Entries;
use super::ids::resolve;
use super::{Access, ReadWrite};
use crate::WriteRefusal;
use crate::db::{self, DB_FILE, docs};
use crate::error::refusal;
use crate::error::{Error, Result};
use crate::record::Listing;
use crate::{
    ENTRIES_DIR, Entry, EntryFilter, EntryId, Proposal, RECORD_EXT, SessionId, Skipped, Store,
};
use std::path::PathBuf;
use time::OffsetDateTime;

impl<A: Access> Entries<'_, A> {
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

    /// Entries matching `filter` among those `keep` accepts, which it sees
    /// before the filter's limit. Reads only.
    pub fn list_where(
        &self,
        filter: &EntryFilter,
        keep: impl FnMut(&Entry) -> bool,
    ) -> Result<EntryList> {
        self.store.list_entries_where(filter, keep)
    }

    /// The entries already recorded from one prompt, by their cite's
    /// `session` and `prompt_sha256`. Reads only.
    pub fn from_prompt(&self, session: &SessionId, prompt_sha256: &str) -> Result<Vec<Entry>> {
        self.store.entries_from_prompt(session, prompt_sha256)
    }
}

/// The writes, on the repository a [`Tx`](super::Tx) gives: a
/// [`Snapshot`](super::Snapshot)'s cannot write.
impl Entries<'_, ReadWrite> {
    /// Erase `entry` from the stored messages it cites and from the archive
    /// tables, in the write under way (`proposals`: the claims before the
    /// forget).
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn erase_residue(
        &self,
        entry: &Entry,
        proposals: &[Proposal],
        now: OffsetDateTime,
    ) -> Result<()> {
        self.store.erase_entry_residue(entry, proposals, now)
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

    /// Delete the handoff `id` in the transaction under way, leaving its
    /// tombstone.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn delete_handoff(&self, id: &EntryId) -> Result<()> {
        self.store
            .with_conn(|conn, db| docs::delete_handoff(conn, db, id))
    }
}

/// What [`Store::list_entries`] found.
#[derive(Debug)]
pub struct EntryList {
    /// The matching entries, oldest first.
    pub listing: Listing<Entry>,
    /// Entry files left out because they did not parse.
    pub skipped: Vec<Skipped>,
}

impl Store {
    /// The file entry `id` lives in, `<root>/entries/<tag>/<id>.md`.
    #[must_use]
    pub(crate) fn entry_path(&self, id: &EntryId) -> PathBuf {
        self.root
            .join(ENTRIES_DIR)
            .join(id.tag().as_str())
            .join(format!("{id}{RECORD_EXT}"))
    }

    /// Entries matching `filter`, oldest first, and how many matched. A
    /// missing store is an empty one.
    pub(crate) fn list_entries(&self, filter: &EntryFilter) -> Result<EntryList> {
        self.list_entries_where(filter, |_| true)
    }

    /// List the entries `keep` accepts, before applying the filter and its
    /// limit.
    pub(crate) fn list_entries_where(
        &self,
        filter: &EntryFilter,
        keep: impl FnMut(&Entry) -> bool,
    ) -> Result<EntryList> {
        let (mut entries, skipped) = self.load_entries()?;
        entries.retain(keep);
        Ok(EntryList {
            listing: filter.select(entries),
            skipped,
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

    /// Every entry, plus those that did not read. Anything other than a
    /// parse failure (a link, loose modes, a newer format, an I/O error)
    /// refuses the whole load.
    pub(crate) fn load_entries(&self) -> Result<(Vec<Entry>, Vec<Skipped>)> {
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

    /// Write `entry` in the write under way. A new entry never replaces
    /// one: ids come from the listing, so a taken id means the listing
    /// missed it, and the write is refused.
    pub(crate) fn write_entry(&self, entry: &Entry, new: bool) -> Result<()> {
        self.with_conn(|conn, path| {
            docs::put_entry(conn, path, entry, new, |e| {
                db::db_error("write entries to", path, e)
            })
        })
    }

    /// Erase `entry` from the stored messages it cites and from the archive
    /// tables, in the forget's own write (`proposals`: the claims before
    /// the forget).
    pub(crate) fn erase_entry_residue(
        &self,
        entry: &Entry,
        proposals: &[Proposal],
        _now: OffsetDateTime,
    ) -> Result<()> {
        // The cited turns' stored messages go first: they hold the words
        // the forget removes, and deleting them again is a no-op.
        self.forget_cited_messages(entry, proposals)?;
        self.with_conn(|conn, path| crate::db::import::scrub_entry(conn, path, entry, proposals))
            .map(drop)
    }

    /// Resolve an exact entry id or a unique prefix of at least four characters.
    pub(crate) fn find_entry(&self, input: &str) -> Result<Entry> {
        let exact = input.parse::<EntryId>().ok();
        if let Some(id) = &exact {
            match self.entry(id) {
                Ok(entry) => return Ok(entry),
                Err(Error::EntryNotFound { .. }) => {}
                Err(e) => return Err(e),
            }
        } else if input.len() < 4
            || !input
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || input
                .rsplit_once('-')
                .is_some_and(|(_, n)| n.starts_with('0'))
        {
            return Err(refusal(
                WriteRefusal::InvalidId,
                "use an entry id or a lowercase unique prefix of at least four characters",
            ));
        }
        let (entries, skipped) = self.load_entries()?;
        let mut ids: Vec<String> = entries.iter().map(|e| e.id.to_string()).collect();
        ids.extend(skipped.iter().filter_map(|s| {
            s.path
                .file_stem()?
                .to_str()?
                .parse::<EntryId>()
                .ok()
                .map(|id| id.to_string())
        }));
        ids.sort();
        ids.dedup();
        let Some(i) = resolve(input, &ids)? else {
            return Err(match exact {
                Some(id) => Error::EntryNotFound {
                    path: self.entry_path(&id),
                    id,
                },
                None => refusal(
                    WriteRefusal::EntryNotFound,
                    format!(
                        "no entry matches '{input}' under {}; run 'scribe entry list' with the same --root to see existing ids",
                        self.root.join(crate::ENTRIES_DIR).display()
                    ),
                ),
            });
        };
        let id = ids[i]
            .parse::<EntryId>()
            .map_err(|e| refusal(WriteRefusal::InvalidId, e.to_string()))?;
        self.entry(&id)
    }
}
