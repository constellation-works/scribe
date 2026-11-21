//! `memory/<id>.md`: the memory items the session pass writes, and the
//! agent writes on them (docs/design/scribe/2_design.md §13.2, §13.4,
//! §13.9).
//!
//! Reads take no lock and write nothing. Every write holds the store lock,
//! replaces one item file whole through `write_atomic`, and rewrites
//! `control.json` at the current format, so a build older than the memory
//! format refuses the store instead of reading it without its memory.

use super::agent::{check_agent_author, check_reason, refusal};
use super::{CONTROL_FILE, RECORD_EXT, Redactor, Skipped, Store, format, fsio, ids};
use crate::memory::consolidate::{self, Change, Judgement};
use crate::memory::promote::{self, Promoted};
use crate::memory::{
    self, Applied, Draft, MemoryId, MemoryItem, MemoryStatus, Provenance, Refusal,
};
use crate::{Author, AuthorName, Error, Result, SessionId, WriteRefusal};
use std::path::PathBuf;
use time::OffsetDateTime;

/// The memory directory's name under the data directory.
pub const MEMORY_DIR: &str = "memory";

/// What a pass's memory write did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryWritten {
    /// Each draft written: a new item or an occurrence of one, by id.
    pub applied: Vec<Applied>,
    /// Drafts refused under the lock, by code.
    pub refused: Vec<Refusal>,
    /// The promotions the drafts caused (§13.4).
    pub promoted: Vec<Promoted>,
}

/// What the scan's memory steps changed (§13.5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Consolidated {
    /// Merges, supersedes and pairs judged unrelated, in label order.
    pub changes: Vec<Change>,
    /// Journalled promotions completed: the items changed.
    pub completed: Vec<MemoryId>,
    /// Rule (b)'s promotions after the merges.
    pub promoted: Vec<Promoted>,
    /// Every item whose file changed, in id order: one audit line each.
    pub items: Vec<MemoryId>,
}

/// The memory a pass window writes with its record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassMemory {
    /// The checked elements, tied to their turns.
    pub drafts: Vec<Draft>,
    /// The session, host and project every occurrence records.
    pub provenance: Provenance,
}

impl Store {
    /// The memory file of `id`.
    #[must_use]
    pub fn memory_path(&self, id: &MemoryId) -> PathBuf {
        self.root.join(MEMORY_DIR).join(format!("{id}{RECORD_EXT}"))
    }

    /// Every memory item, in id order, plus the files that did not parse.
    /// None for a missing directory or store. Reads only.
    ///
    /// # Errors
    ///
    /// Anything but a parse failure: a link, loose modes, a newer format,
    /// an I/O error.
    pub fn memory_items(&self) -> Result<(Vec<MemoryItem>, Vec<Skipped>)> {
        let mut items = Vec::new();
        let mut skipped = Vec::new();
        let dir = self.root.join(MEMORY_DIR);
        if !self.root_exists()? || !fsio::inspect_subdir(&dir)? {
            return Ok((items, skipped));
        }
        for name in super::visible(fsio::list_dir(&dir)?) {
            let Some(stem) = name.strip_suffix(RECORD_EXT) else {
                continue;
            };
            let path = dir.join(&name);
            if !fsio::inspect_file(&path)? {
                continue;
            }
            match format::decode_memory(&path, &fsio::read(&path)?, stem) {
                Ok(item) => items.push(item),
                Err(error @ Error::Corrupt { .. }) => skipped.push(Skipped { path, error }),
                Err(other) => return Err(other),
            }
        }
        items.sort_by_key(|i| i.id.number());
        Ok((items, skipped))
    }

    /// The item `input` names: an id, or a unique prefix of at least four
    /// characters. Reads only.
    ///
    /// # Errors
    ///
    /// `memory_not_found`, `id_ambiguous`, `usage_error`, and store
    /// failures.
    pub fn resolve_memory(&self, input: &str) -> Result<MemoryItem> {
        let (items, skipped) = self.memory_items()?;
        let ids: Vec<String> = items.iter().map(|i| i.id.to_string()).collect();
        match ids::resolve(input, &ids)? {
            Some(i) => Ok(items[i].clone()),
            None => {
                // A file that did not parse is reported as itself, not as missing.
                if let Some(bad) = skipped.into_iter().find(|s| {
                    s.path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|stem| stem == input)
                }) {
                    return Err(bad.error);
                }
                Err(refusal(
                    WriteRefusal::MemoryNotFound,
                    format!(
                        "no memory item {input} under {}; run 'scribe memory show --include-off-page'",
                        self.root.join(MEMORY_DIR).display()
                    ),
                ))
            }
        }
    }

    /// Write a pass window's memory under the lock the caller holds
    /// (§13.3): each draft checked again (its quote the user's own span of
    /// its turn, its section switched on in the config on disk, its text
    /// unchanged by the redactor), then folded into the items
    /// ([`memory::apply`]); each changed item replaced whole, then
    /// `control.json` rewritten at the current format.
    pub(super) fn write_pass_memory(
        &self,
        pass: &PassMemory,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<MemoryWritten> {
        let mut written = MemoryWritten::default();
        if pass.drafts.is_empty() {
            return Ok(written);
        }
        let config = self.config()?;
        let (mut items, skipped) = self.memory_items()?;
        if let Some(bad) = skipped.into_iter().next() {
            // Without every item a repeat could open a second one.
            return Err(bad.error);
        }
        let before = items.clone();
        for draft in &pass.drafts {
            let element = &draft.checked;
            if !crate::pass::own_span(&draft.turn, &element.quote) {
                written.refused.push(Refusal::Unverified);
                continue;
            }
            if !config.memory.section_on(element.section) {
                written.refused.push(Refusal::SectionOff);
                continue;
            }
            if redactor.mask(&element.statement) != element.statement
                || redactor.mask(&element.quote) != element.quote
            {
                written.refused.push(Refusal::Sensitive);
                continue;
            }
            if let Some(promote) = &element.promote
                && redactor.mask(promote) != *promote
            {
                written.refused.push(Refusal::Sensitive);
                continue;
            }
            match promote::write(&mut items, draft, &pass.provenance, &config.memory, now) {
                Ok(done) => {
                    written.applied.push(done.applied);
                    written.promoted.extend(done.promoted);
                }
                Err(refused) => written.refused.push(refused),
            }
        }
        self.write_changed(&before, &items)?;
        Ok(written)
    }

    /// Write every item of `items` that differs from `before`, those that
    /// gained (a new item, a milestone with its journal, a merge's target)
    /// first, so a crash between the files leaves the occurrences on both
    /// rather than on neither, and the next scan completes the change.
    /// The caller holds the lock.
    fn write_changed(&self, before: &[MemoryItem], items: &[MemoryItem]) -> Result<Vec<MemoryId>> {
        let mut changed: Vec<(bool, &MemoryItem)> = items
            .iter()
            .filter(|i| !before.contains(i))
            .map(|i| {
                let old = before.iter().find(|b| b.id == i.id);
                let gained = old.is_none_or(|b| i.occurrences.len() > b.occurrences.len());
                (!gained, i)
            })
            .collect();
        changed.sort_by_key(|(later, i)| (*later, i.id.number()));
        for (_, item) in &changed {
            self.write_memory(item)?;
        }
        let mut ids: Vec<MemoryId> = changed.into_iter().map(|(_, i)| i.id.clone()).collect();
        ids.sort_by_key(MemoryId::number);
        Ok(ids)
    }

    /// The scan's memory steps under the store lock (§13.5): journalled
    /// promotions completed, the surviving labels applied (merge,
    /// supersede, unrelated), then rule (b) recounted. Every change is by
    /// `{kind: scribe, name: consistency}`. Applying the same labels again
    /// changes nothing.
    ///
    /// # Errors
    ///
    /// A memory file that does not parse, and store failures.
    pub fn consolidate_memory(
        &self,
        judgements: &[Judgement],
        now: OffsetDateTime,
    ) -> Result<Consolidated> {
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("consistency scan")?;
        let config = self.config()?;
        let (mut items, skipped) = self.memory_items()?;
        if let Some(bad) = skipped.into_iter().next() {
            return Err(bad.error);
        }
        let before = items.clone();
        let by = Author::consistency();
        let completed = promote::complete(&mut items, &by, now);
        let changes = consolidate::apply(&mut items, judgements, &by, now);
        let promoted = promote::recount(&mut items, &config.memory.promote, &by, now)
            .into_iter()
            .map(|(item, to)| Promoted {
                item: Some(item),
                to,
                rule: promote::PromotionRule::RecursInSessions,
                profile_to: None,
                profile_from: None,
            })
            .collect();
        let items = self.write_changed(&before, &items)?;
        Ok(Consolidated {
            changes,
            completed,
            promoted,
            items,
        })
    }

    /// Replace `item`'s file and stamp `control.json` with the current
    /// format. The caller holds the lock.
    fn write_memory(&self, item: &MemoryItem) -> Result<()> {
        self.prepare_subdir(&self.root.join(MEMORY_DIR))?;
        let path = self.memory_path(&item.id);
        fsio::write_atomic(&path, &format::encode_memory(&path, item)?)?;
        let control = self.root.join(CONTROL_FILE);
        let state = self.control()?;
        fsio::write_atomic(&control, &format::encode_control(&control, &state)?)
    }

    /// Mark a `now` item raised in `session` by the agent `author`
    /// (`memory ack`, §13.4). The same session and author again changes
    /// nothing.
    ///
    /// # Errors
    ///
    /// `reserved_author`, `memory_not_found`, `memory_not_active`,
    /// `memory_not_now`, and store failures.
    pub fn ack_memory(
        &self,
        id: &str,
        session: SessionId,
        author: AuthorName,
        now: OffsetDateTime,
    ) -> Result<(MemoryItem, bool)> {
        check_agent_author(&author)?;
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("memory ack")?;
        let mut item = self.resolve_memory(id)?;
        active(&item)?;
        if item.section != memory::Section::Now {
            return Err(refusal(
                WriteRefusal::MemoryNotNow,
                format!(
                    "{} is a {} item; only now items are acknowledged",
                    item.id, item.section
                ),
            ));
        }
        let author = Author::agent(author);
        if item
            .acknowledged
            .as_ref()
            .is_some_and(|a| a.session == session && a.author == author)
        {
            return Ok((item, false));
        }
        item.acknowledged = Some(memory::Acknowledged {
            at: now,
            session,
            author: author.clone(),
        });
        item.updated = now;
        item.updated_by = author;
        self.write_memory(&item)?;
        Ok((item, true))
    }

    /// Retire an item for the agent `author`, with its redacted `reason`
    /// (§13.9): it leaves the page, lists and `ask`, its text kept. An
    /// item already retired changes nothing.
    ///
    /// # Errors
    ///
    /// `reserved_author`, `invalid_text`, `memory_not_found`,
    /// `memory_not_active` (a forgotten item), and store failures.
    pub fn retire_memory(
        &self,
        id: &str,
        author: AuthorName,
        reason: &str,
        redactor: &Redactor,
        now: OffsetDateTime,
    ) -> Result<(MemoryItem, bool)> {
        check_agent_author(&author)?;
        check_reason(reason)?;
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("memory retire")?;
        let mut item = self.resolve_memory(id)?;
        match item.status {
            MemoryStatus::Retired => return Ok((item, false)),
            MemoryStatus::Forgotten
            | MemoryStatus::Merged
            | MemoryStatus::Superseded
            | MemoryStatus::Promoted => return Err(not_active(&item)),
            MemoryStatus::Active => {}
        }
        item.status = MemoryStatus::Retired;
        item.retirement_reason = Some(redactor.mask(reason.trim()));
        item.updated = now;
        item.updated_by = Author::agent(author);
        self.write_memory(&item)?;
        Ok((item, true))
    }

    /// Forget an item for good for the agent `author` (§13.9): with
    /// `occurrence`, only that occurrence's quote; otherwise the statement,
    /// every quote and any reason, the item `forgotten`, and so are the
    /// items holding the same words ([`memory::linked`]). No file is
    /// removed; id, section, dates and authors stay. Forgetting what is
    /// already forgotten changes nothing.
    ///
    /// # Errors
    ///
    /// `reserved_author`, `memory_not_found` (also for an occurrence the
    /// item does not have), and store failures.
    pub fn forget_memory(
        &self,
        id: &str,
        occurrence: Option<u32>,
        author: AuthorName,
        now: OffsetDateTime,
    ) -> Result<(MemoryItem, bool)> {
        check_agent_author(&author)?;
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("memory forget")?;
        let mut item = self.resolve_memory(id)?;
        let by = Author::agent(author);
        let changed = match occurrence {
            None => {
                // The items holding the same words go with it: those merged
                // into it, and the other side of a promotion.
                let (mut items, _) = self.memory_items()?;
                for index in memory::linked(&items, &item.id) {
                    let linked = &mut items[index];
                    if memory::forget(linked, by.clone(), now) {
                        self.write_memory(linked)?;
                    }
                }
                memory::forget(&mut item, by, now)
            }
            Some(n) => memory::forget_occurrence(&mut item, n, by, now).ok_or_else(|| {
                refusal(
                    WriteRefusal::MemoryNotFound,
                    format!(
                        "{} has no occurrence {n}; run 'scribe memory show {}'",
                        item.id, item.id
                    ),
                )
            })?,
        };
        if changed {
            self.write_memory(&item)?;
        }
        Ok((item, changed))
    }
}

fn active(item: &MemoryItem) -> Result<()> {
    if item.status == MemoryStatus::Active {
        Ok(())
    } else {
        Err(not_active(item))
    }
}

fn not_active(item: &MemoryItem) -> Error {
    refusal(
        WriteRefusal::MemoryNotActive,
        format!(
            "{} is {}; only an active item changes",
            item.id, item.status
        ),
    )
}
