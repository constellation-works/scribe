//! The entry repository: notes, findings, handoffs, decisions and open
//! items in `scribe.db`, their listing and lookup, and a forget's erasure
//! of an entry or of a session's handoffs.

use super::Entries;
use super::ids::resolve;
use crate::checks::refusal;
use crate::db::{self, DB_FILE, docs};
use crate::error::{Error, Result};
use crate::record::Listing;
use crate::{
    ENTRIES_DIR, Entry, EntryFilter, EntryId, Proposal, RECORD_EXT, SessionId, Skipped, Store,
};
use scribe_domain::WriteRefusal;
use std::collections::BTreeSet;
use std::path::PathBuf;
use time::OffsetDateTime;

impl Entries<'_> {
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

    /// Write `entry` in the transaction under way; `new` refuses to replace
    /// an existing row.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put(&self, entry: &Entry, new: bool) -> Result<()> {
        self.store.write_entry(entry, new)
    }

    /// Entries matching `filter` among those of the `members` sessions, when
    /// given. Reads only.
    pub fn list_scoped(
        &self,
        filter: &EntryFilter,
        members: Option<&BTreeSet<SessionId>>,
    ) -> Result<EntryList> {
        self.store.list_entries_scoped(filter, members)
    }

    /// Active `handoff` entries referencing `session:<id>`, newest first.
    /// Reads only.
    pub fn handoffs_for_session(&self, id: &SessionId) -> Result<Vec<Entry>> {
        self.store.handoffs_for_session(id)
    }

    /// The entries already recorded from one prompt, by their cite's
    /// `session` and `prompt_sha256`. Reads only.
    pub fn from_prompt(&self, session: &SessionId, prompt_sha256: &str) -> Result<Vec<Entry>> {
        self.store.entries_from_prompt(session, prompt_sha256)
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

    /// Erase session handoffs and withdraw claims in the same write.
    pub(crate) fn erase_session_documents(
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
