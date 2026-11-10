//! Persistence: the file-backed scribe store and everything that touches
//! disk.
//!
//! The rest of the crate is business logic with no I/O (`entry`, `tag`,
//! `session`, `proposal`, `config`, `verbatim`); this module is the only one
//! that reads or writes files, so a second backend replaces this module and
//! nothing else. It is split by responsibility:
//!
//! - this file, the [`Store`] facade: walk the layout, load, select;
//! - `format`: the persisted documents, their format versions and upgrades;
//! - `audit`: the audit log's record shape;
//! - `redact`: the one redactor every persisted free-text field passes;
//! - `fsio`: durable writes, appends, locking and private-state checks,
//!   mechanism only.
//!
//! The layout (docs/design/scribe/2_design.md §3):
//!
//! ```text
//! <root>/config.toml                        read-only to scribe
//! <root>/entries/<tag>/<id>.md              one entry per file
//! <root>/sessions/<yyyy>/<mm>/<id>.md       one session record per file
//! <root>/control.json                       the on/off state
//! <root>/proposals.json                     the consistency proposals
//! <root>/audit.jsonl                        one line per mutating command
//! <root>/.lock
//! ```
//!
//! Reads take no lock and write nothing (STD-01 R31): every file is only
//! ever replaced by an atomic rename, so a reader sees a whole old version or
//! a whole new one, and the CLI works on a read-only root. Every load refuses
//! state that is a link, another user's, or writable by others
//! (STD-05 R7–R9), and a newer format (STD-03 R10). A record that does not
//! parse is refused naming its file; a listing skips it and reports it in
//! [`Skipped`], so one bad file does not hide the rest (4_decisions.md, "One
//! malformed record makes its file corrupt").

mod audit;
mod format;
mod fsio;
mod rebuild;
mod redact;
pub use rebuild::RebuildGuard;

pub use audit::{AUDIT_FILE, AuditEvent, AuditStatus};
pub use redact::{MIN_SECRET_CHARS, REDACTED_FIELDS, Redactor, unresolved_redact_env};

use crate::config::Config;
use crate::consistency::{self, Draft, Reapplied, Settle, Settled};
use crate::control;
use crate::entry::{Cite, Entry, EntryFilter, EntryId, NewEntry};
use crate::error::{Error, Result};
use crate::proposal::{Proposal, ProposalId};
use crate::record::Listing;
use crate::session::{
    self as session_mod, Child, ControlState, SessionFilter, SessionId, SessionLink, SessionRecord,
};
use crate::tag::{self, TagSummary};
use fsio::LockGuard;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The configuration file's name under the data directory.
pub const CONFIG_FILE: &str = "config.toml";
/// The entries directory's name under the data directory.
pub const ENTRIES_DIR: &str = "entries";
/// The session records directory's name under the data directory.
pub const SESSIONS_DIR: &str = "sessions";
/// The control document's name under the data directory.
pub const CONTROL_FILE: &str = "control.json";
/// The proposals document's name under the data directory.
pub const PROPOSALS_FILE: &str = "proposals.json";

/// The lock file's name under the data directory.
const LOCK_FILE: &str = ".lock";
/// A record file's extension.
const RECORD_EXT: &str = ".md";

/// How long a writer waits for another writer before refusing (STD-03 R7).
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(10);

/// A record file a listing left out because it did not parse.
#[derive(Debug)]
pub struct Skipped {
    /// The file.
    pub path: PathBuf,
    /// Why: always [`Error::Corrupt`], which names the file.
    pub error: Error,
}

/// What [`Store::list_entries`] found.
#[derive(Debug)]
pub struct EntryList {
    /// The matching entries, oldest first.
    pub listing: Listing<Entry>,
    /// Entry files left out because they did not parse.
    pub skipped: Vec<Skipped>,
}

/// What [`Store::list_sessions`] found.
#[derive(Debug)]
pub struct SessionList {
    /// The matching session records, oldest first.
    pub listing: Listing<SessionRecord>,
    /// Every session's children, derived from all records' links (not only
    /// the listed ones), by the id they link to.
    pub children: BTreeMap<SessionId, Vec<Child>>,
    /// Every record's title, by id, for naming a linked session.
    pub titles: BTreeMap<SessionId, String>,
    /// Every session's on/off switch, to show beside its record.
    pub control: ControlState,
    /// Inactive sessions the filter left out that match its other
    /// criteria; `0` when it includes inactive ones.
    pub inactive_hidden: usize,
    /// Record files left out because they did not parse.
    pub skipped: Vec<Skipped>,
}

/// One session record with its switch.
#[derive(Debug)]
pub struct SessionView {
    /// The record.
    pub record: SessionRecord,
    /// Its switch in `control.json`, if it has one.
    pub control: Option<crate::session::SessionControl>,
    /// The sessions that link to it, derived from their records.
    pub children: Vec<Child>,
    /// The titles of the sessions its links point at that have a record.
    pub linked_titles: BTreeMap<SessionId, String>,
}

/// What [`Store::list_tags`] found.
#[derive(Debug)]
pub struct TagList {
    /// Every configured tag and every tag with entries, by name.
    pub tags: Vec<TagSummary>,
    /// Entry files left out of the counts because they did not parse.
    pub skipped: Vec<Skipped>,
}

/// What [`Store::capture`] did with one draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Captured {
    /// It was written as a new entry.
    Added(Entry),
    /// An entry for the same `#scribe` line already existed (a retried or
    /// replayed hook); nothing was written.
    Existing(Entry),
}

impl Captured {
    /// The entry written or found.
    #[must_use]
    pub fn entry(&self) -> &Entry {
        match self {
            Self::Added(entry) | Self::Existing(entry) => entry,
        }
    }
}

/// A scribe store rooted at one data directory.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
    lock_wait: Duration,
}

impl Store {
    /// A store at `root`. Performs no I/O. The caller resolves `root`
    /// (STD-02 R3): this crate never reads the environment or the home
    /// directory.
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            lock_wait: DEFAULT_LOCK_WAIT,
        }
    }

    /// Override how long a writer waits for the lock.
    #[must_use]
    pub fn with_lock_wait(mut self, wait: Duration) -> Self {
        self.lock_wait = wait;
        self
    }

    /// The data directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The audit log, `<root>/audit.jsonl`.
    #[must_use]
    pub fn audit_path(&self) -> PathBuf {
        self.root.join(AUDIT_FILE)
    }

    /// The file entry `id` lives in, `<root>/entries/<tag>/<id>.md`.
    #[must_use]
    pub fn entry_path(&self, id: &EntryId) -> PathBuf {
        self.root
            .join(ENTRIES_DIR)
            .join(id.tag().as_str())
            .join(format!("{id}{RECORD_EXT}"))
    }

    /// The configuration: `config.toml` parsed, or the defaults when there
    /// is none. A bad key or value is refused, naming it.
    pub fn config(&self) -> Result<Config> {
        let path = self.root.join(CONFIG_FILE);
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(Config::default());
        }
        let bytes = fsio::read(&path)?;
        let text = std::str::from_utf8(&bytes).map_err(|e| Error::corrupt(&path, e))?;
        Config::parse(text).map_err(|source| Error::Config { path, source })
    }

    /// Entries matching `filter`, oldest first, and how many matched. A
    /// missing store is an empty one.
    pub fn list_entries(&self, filter: &EntryFilter) -> Result<EntryList> {
        let (entries, skipped) = self.load_entries()?;
        Ok(EntryList {
            listing: filter.select(entries),
            skipped,
        })
    }

    /// The entry with this id.
    pub fn entry(&self, id: &EntryId) -> Result<Entry> {
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

    /// Session records matching `filter`, oldest first, with the on/off
    /// state beside them.
    pub fn list_sessions(&self, filter: &SessionFilter) -> Result<SessionList> {
        let control = self.control()?;
        let mut records = Vec::new();
        let mut skipped = Vec::new();
        for file in self.session_files()? {
            match file.load() {
                Ok(record) => records.push(record),
                Err(error @ Error::Corrupt { .. }) => skipped.push(Skipped {
                    path: file.path,
                    error,
                }),
                Err(other) => return Err(other),
            }
        }
        let inactive_hidden = if filter.include_inactive {
            0
        } else {
            let any = SessionFilter {
                include_inactive: true,
                ..filter.clone()
            };
            records
                .iter()
                .filter(|r| !r.active && any.matches(r))
                .count()
        };
        Ok(SessionList {
            children: session_mod::children(&records),
            titles: records
                .iter()
                .map(|r| (r.id.clone(), r.title.clone()))
                .collect(),
            listing: filter.select(records),
            inactive_hidden,
            control,
            skipped,
        })
    }

    /// The session record with this id, its switch, the sessions that link
    /// to it and the titles of those it links to. Other records that do not
    /// parse are left out of the children.
    pub fn session(&self, id: &SessionId) -> Result<SessionView> {
        let control = self.control()?;
        let files = self.session_files()?;
        let file = files
            .iter()
            .find(|f| f.stem == id.as_str())
            .ok_or_else(|| Error::SessionNotFound {
                id: id.clone(),
                path: self.root.join(SESSIONS_DIR),
            })?;
        let record = file.load()?;
        let mut others = Vec::new();
        for other in files.iter().filter(|f| f.stem != id.as_str()) {
            match other.load() {
                Ok(r) => others.push(r),
                Err(Error::Corrupt { .. }) => {}
                Err(err) => return Err(err),
            }
        }
        let linked_titles = others
            .iter()
            .filter(|r| record.links.iter().any(|l| l.session == r.id))
            .map(|r| (r.id.clone(), r.title.clone()))
            .collect();
        others.push(record.clone());
        let children = session_mod::children(&others)
            .remove(id)
            .unwrap_or_default();
        Ok(SessionView {
            record,
            control: control.get(id).cloned(),
            children,
            linked_titles,
        })
    }

    /// Link `session` to another session, under the store lock
    /// ([`control::link`]): when `on`, switch it on too. A new link is
    /// added to the session's record when it has one (its other fields,
    /// `updated` included, unchanged), else kept on its switch until the
    /// first pass writes the record. Refused links write nothing.
    ///
    /// # Errors
    ///
    /// [`Error::Link`] for a refused link; a store error otherwise. The
    /// session's own record failing to parse is [`Error::Corrupt`].
    pub fn link_session(
        &self,
        session: &SessionId,
        link: &SessionLink,
        on: bool,
        now: time::OffsetDateTime,
    ) -> Result<control::Linked> {
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("link")?;
        let mut own: Option<SessionRecord> = None;
        let mut graph: BTreeMap<SessionId, Vec<SessionLink>> = BTreeMap::new();
        for file in self.session_files()? {
            match file.load() {
                Ok(record) => {
                    graph.insert(record.id.clone(), record.links.clone());
                    if &record.id == session {
                        own = Some(record);
                    }
                }
                Err(err @ Error::Corrupt { .. }) if file.stem == session.as_str() => {
                    return Err(err);
                }
                Err(Error::Corrupt { .. }) => {
                    // Known by id, its links unreadable.
                    if let Ok(id) = file.stem.parse() {
                        graph.entry(id).or_default();
                    }
                }
                Err(other) => return Err(other),
            }
        }
        // A refused link changes nothing, so nothing is written.
        let linked =
            self.change_control(|state| control::link(state, &graph, session, link, on, now))??;
        if linked.write_record
            && let Some(mut record) = own
        {
            record.links.push(link.clone());
            let path = self.session_path(&record);
            fsio::write_atomic(&path, &format::encode_session(&path, &record)?)?;
        }
        Ok(linked)
    }

    /// The on/off state; empty when there is no `control.json`.
    pub fn control(&self) -> Result<ControlState> {
        let path = self.root.join(CONTROL_FILE);
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(ControlState::default());
        }
        format::decode_control(&path, &fsio::read(&path)?)
    }

    /// Every proposal, by id; none when there is no `proposals.json`.
    pub fn proposals(&self) -> Result<Vec<Proposal>> {
        let path = self.root.join(PROPOSALS_FILE);
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(Vec::new());
        }
        let mut proposals = format::decode_proposals(&path, &fsio::read(&path)?)?;
        proposals.sort_by_key(|p| p.id);
        Ok(proposals)
    }

    /// Every configured tag and every tag that has entries, with counts.
    pub fn list_tags(&self) -> Result<TagList> {
        let config = self.config()?;
        let (entries, skipped) = self.load_entries()?;
        Ok(TagList {
            tags: tag::summarize(&config.tags, &entries),
            skipped,
        })
    }

    /// Record each draft as a new entry, once.
    ///
    /// Under the store lock, each draft is checked against the existing
    /// entries by its cite's `(session, prompt_sha256, line)`: a match is
    /// returned as [`Captured::Existing`] and nothing is written, so a
    /// retried or replayed hook never writes twice. Otherwise the draft gets
    /// the next id in its tag (one past the highest, counting files that do
    /// not parse, so a number is never reused), its free text passes
    /// `redactor`, and it is written with [`fsio::write_atomic`]. Missing
    /// directories are created owner-only.
    pub fn capture(&self, drafts: Vec<NewEntry>, redactor: &Redactor) -> Result<Vec<Captured>> {
        if drafts.is_empty() {
            return Ok(Vec::new());
        }
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("capture")?;
        let (mut entries, skipped) = self.load_entries()?;
        let mut ids: Vec<EntryId> = entries.iter().map(|e| e.id.clone()).collect();
        ids.extend(
            skipped
                .iter()
                .filter_map(|s| s.path.file_stem()?.to_str()?.parse().ok()),
        );
        let mut out = Vec::with_capacity(drafts.len());
        for draft in drafts {
            if let Some(found) = entries.iter().find(|e| e.cite.same_line(&draft.cite)) {
                out.push(Captured::Existing(found.clone()));
                continue;
            }
            let id = EntryId::next_in(&draft.tag, &ids);
            let mut entry = draft.into_entry(id.clone());
            redactor.entry(&mut entry);
            self.write_entry(&entry, true)?;
            ids.push(id);
            entries.push(entry.clone());
            out.push(Captured::Added(entry));
        }
        Ok(out)
    }

    /// The entries already recorded from one prompt: `session` and
    /// `prompt_sha256` as their cite has them. Reads only, so the hook can
    /// echo a replayed form-3 line without asking the model again.
    pub fn entries_from_prompt(
        &self,
        session: &SessionId,
        prompt_sha256: &str,
    ) -> Result<Vec<Entry>> {
        Ok(self
            .load_entries()?
            .0
            .into_iter()
            .filter(|e| &e.cite.session == session && e.cite.prompt_sha256 == prompt_sha256)
            .collect())
    }

    /// Whether any entry from `session` still waits for its cite's message
    /// uuid. Reads only, so the `Stop` hook, which runs after every reply,
    /// can return without locking or writing when nothing waits.
    pub fn has_unfilled_cites(&self, session: &SessionId) -> Result<bool> {
        Ok(self.load_entries()?.0.iter().any(|e| unfilled(e, session)))
    }

    /// Under the store lock, fill the cite's message uuid of every entry
    /// from `session` that has none yet with what `find` returns for its
    /// cite; `updated` becomes `now`. Returns the ids filled, oldest first.
    pub fn fill_cite_messages(
        &self,
        session: &SessionId,
        now: time::OffsetDateTime,
        find: impl Fn(&Cite) -> Option<String>,
    ) -> Result<Vec<EntryId>> {
        if !self.root_exists()? {
            return Ok(Vec::new());
        }
        let _lock = self.lock("fill cite")?;
        let (mut entries, _) = self.load_entries()?;
        entries.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
        let mut filled = Vec::new();
        for mut entry in entries.into_iter().filter(|e| unfilled(e, session)) {
            let Some(message) = find(&entry.cite) else {
                continue;
            };
            entry.cite.message = Some(message);
            entry.updated = now;
            self.write_entry(&entry, false)?;
            filled.push(entry.id);
        }
        Ok(filled)
    }

    /// Change the on/off state under the store lock: `change` gets the
    /// current state and its result is returned. `control.json` is
    /// rewritten only when the state changed. The data directory is created
    /// owner-only when absent, so a caller that may change nothing (the
    /// `Stop` hook) reads [`Store::control`] first and calls this only when
    /// a change is likely.
    pub fn update_control<T>(&self, change: impl FnOnce(&mut ControlState) -> T) -> Result<T> {
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("control")?;
        self.change_control(change)
    }

    /// [`Store::update_control`] for a caller that holds the lock.
    fn change_control<T>(&self, change: impl FnOnce(&mut ControlState) -> T) -> Result<T> {
        let mut state = self.control()?;
        let before = state.clone();
        let out = change(&mut state);
        if state != before {
            let path = self.root.join(CONTROL_FILE);
            fsio::write_atomic(&path, &format::encode_control(&path, &state)?)?;
        }
        Ok(out)
    }

    /// The file a session record lives in,
    /// `<root>/sessions/<yyyy>/<mm>/<id>.md`, by the month it started.
    #[must_use]
    pub fn session_path(&self, record: &SessionRecord) -> PathBuf {
        self.root
            .join(SESSIONS_DIR)
            .join(format!("{:04}", record.started.year()))
            .join(format!("{:02}", u8::from(record.started.month())))
            .join(format!("{}{RECORD_EXT}", record.id))
    }

    /// Finish a session pass under the store lock: write `record` (when the
    /// pass produced one) whole with [`fsio::write_atomic`], its free text
    /// masked by `redactor`, then settle the session's switch with
    /// [`control::settle`]. Returns the record as written.
    pub fn settle_pass(
        &self,
        session: &SessionId,
        record: Option<SessionRecord>,
        ran_final: bool,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        self.write_pass(session, record, Some(ran_final), redactor)
    }

    /// Persist one intermediate window under the store lock, redacted and
    /// atomically replaced. Keep the pass due until its last window settles.
    pub fn checkpoint_pass(
        &self,
        record: SessionRecord,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        let session = record.id.clone();
        self.write_pass(&session, Some(record), None, redactor)
    }

    fn write_pass(
        &self,
        session: &SessionId,
        record: Option<SessionRecord>,
        ran_final: Option<bool>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("session pass")?;
        // An explicit pass can start when not due. Publish the obligation
        // before the checkpoint, so interruption cannot strand its remainder.
        if ran_final.is_none() {
            self.change_control(|state| {
                if let Some(control) = state.sessions.get_mut(session).filter(|c| c.on) {
                    control.due = true;
                }
            })?;
        }
        let written = match record {
            Some(mut record) => {
                // A link written while the model ran, or one waiting on the
                // switch for the first record, is kept: never lost to a pass.
                let current = self
                    .session_files()?
                    .into_iter()
                    .find(|f| f.stem == session.as_str())
                    .map(|f| f.load())
                    .transpose()?;
                if let Some(current) = &current {
                    for sealed in current
                        .log
                        .iter()
                        .filter(|s| s.state == crate::SegmentState::Sealed)
                    {
                        if record.segment(sealed.n.get()) != Some(sealed) {
                            return Err(Error::SealedSegmentChanged {
                                id: session.clone(),
                                n: sealed.n.get(),
                            });
                        }
                    }
                    merge_links(&mut record.links, &current.links);
                }
                let pending = self
                    .control()?
                    .get(session)
                    .map(|c| c.pending_links.clone())
                    .unwrap_or_default();
                merge_links(&mut record.links, &pending);
                redactor.session(&mut record);
                // A changed redaction configuration cannot rewrite sealed history.
                if let Some(current) = current {
                    for sealed in current
                        .log
                        .into_iter()
                        .filter(|s| s.state == crate::SegmentState::Sealed)
                    {
                        if let Some(segment) = record.log.iter_mut().find(|s| s.n == sealed.n) {
                            *segment = sealed;
                        }
                    }
                }
                let path = self.session_path(&record);
                let mut dir = self.root.join(SESSIONS_DIR);
                self.prepare_subdir(&dir)?;
                for part in [
                    format!("{:04}", record.started.year()),
                    format!("{:02}", u8::from(record.started.month())),
                ] {
                    dir = dir.join(part);
                    self.prepare_subdir(&dir)?;
                }
                fsio::write_atomic(&path, &format::encode_session(&path, &record)?)?;
                // Only once the record holds them: the links are stored once.
                if !pending.is_empty() {
                    self.change_control(|state| {
                        if let Some(control) = state.sessions.get_mut(session) {
                            control.pending_links.retain(|l| !pending.contains(l));
                        }
                    })?;
                }
                Some(record)
            }
            None => None,
        };
        if let Some(ran_final) = ran_final {
            self.change_control(|state| control::settle(state, session, ran_final))?;
        }
        Ok(written)
    }

    /// Re-apply, under the store lock, every approved proposal whose older
    /// entry is still `active` ([`consistency::reapply`]): an approval
    /// writes `proposals.json` and then the entry, and this repairs a crash
    /// between the two. Writes only the entries it changes.
    pub fn reapply_approved(&self, now: time::OffsetDateTime) -> Result<Vec<Reapplied>> {
        if !self.root_exists()? {
            return Ok(Vec::new());
        }
        let proposals = self.proposals()?;
        let (entries, _) = self.load_entries()?;
        let mut probe = entries;
        if consistency::reapply(&proposals, &mut probe, now).is_empty() {
            return Ok(Vec::new());
        }
        let _lock = self.lock("consistency scan")?;
        let proposals = self.proposals()?;
        let (mut entries, _) = self.load_entries()?;
        let done = consistency::reapply(&proposals, &mut entries, now);
        for change in &done {
            if let Some(entry) = entries.iter().find(|e| e.id == change.entry) {
                self.write_entry(entry, false)?;
            }
        }
        Ok(done)
    }

    /// Open one proposal per draft, under the store lock: a draft whose
    /// pair (or stale entry) already has a proposal in any state, or whose
    /// entries are no longer all `active`, is skipped
    /// ([`consistency::is_settled_or_gone`], checked again here because a
    /// settle may have run since the drafts were made). The rest are
    /// numbered `p-<n>` past the highest id, their free text masked by
    /// `redactor`, and `proposals.json` is replaced whole. Returns the
    /// proposals opened, by id; writes nothing when there are none.
    pub fn open_proposals(
        &self,
        drafts: Vec<Draft>,
        now: time::OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Vec<Proposal>> {
        if drafts.is_empty() {
            return Ok(Vec::new());
        }
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("consistency scan")?;
        let mut proposals = self.proposals()?;
        let (entries, _) = self.load_entries()?;
        let mut next = proposals.iter().map(|p| p.id.number()).max().unwrap_or(0);
        let mut opened = Vec::new();
        for draft in drafts {
            if consistency::is_settled_or_gone(&draft, &proposals, &entries) {
                continue;
            }
            next = next.saturating_add(1);
            let Ok(id) = ProposalId::try_from(format!("p-{next}")) else {
                continue;
            };
            let mut proposal = draft.into_proposal(id, now);
            redactor.proposal(&mut proposal);
            proposals.push(proposal.clone());
            opened.push(proposal);
        }
        if !opened.is_empty() {
            self.write_proposals(&proposals)?;
        }
        Ok(opened)
    }

    /// Apply one settling instruction typed in the turn `cite`, under the
    /// store lock ([`consistency::settle`]). `proposals.json` is written
    /// first and then the entry, so a crash between them leaves an approved
    /// proposal the next scan re-applies; only what changed is written.
    pub fn settle(
        &self,
        action: &Settle,
        cite: &Cite,
        now: time::OffsetDateTime,
    ) -> Result<Settled> {
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("settle")?;
        let mut proposals = self.proposals()?;
        let (mut entries, _) = self.load_entries()?;
        let before_proposals = proposals.clone();
        let before_entries = entries.clone();
        let settled = consistency::settle(action, &mut proposals, &mut entries, cite, now);
        if proposals != before_proposals {
            self.write_proposals(&proposals)?;
        }
        for (entry, before) in entries.iter().zip(&before_entries) {
            if entry != before {
                self.write_entry(entry, false)?;
            }
        }
        Ok(settled)
    }

    /// Replace `proposals.json` whole. The caller holds the lock.
    fn write_proposals(&self, proposals: &[Proposal]) -> Result<()> {
        let path = self.root.join(PROPOSALS_FILE);
        fsio::write_atomic(&path, &format::encode_proposals(&path, proposals)?)
    }

    /// Write `entry` to its file, creating its directories owner-only. A new
    /// entry never replaces a file: ids come from the listing, so an
    /// existing file means the listing missed it, and the write is refused
    /// rather than overwrite it. The caller holds the lock.
    fn write_entry(&self, entry: &Entry, new: bool) -> Result<()> {
        let dir = self.root.join(ENTRIES_DIR);
        self.prepare_subdir(&dir)?;
        self.prepare_subdir(&dir.join(entry.tag.as_str()))?;
        let path = self.entry_path(&entry.id);
        if new && fsio::inspect_file(&path)? {
            return Err(Error::io(
                "create",
                path,
                io::Error::from(io::ErrorKind::AlreadyExists),
            ));
        }
        fsio::write_atomic(&path, &format::encode_entry(&path, entry)?)
    }

    fn prepare_dir(&self, dir: &Path) -> Result<()> {
        if !fsio::inspect_dir(dir)? {
            fsio::create_private_dir(dir)?;
        }
        Ok(())
    }

    fn prepare_subdir(&self, dir: &Path) -> Result<()> {
        if !fsio::inspect_subdir(dir)? {
            fsio::create_private_dir(dir)?;
        }
        Ok(())
    }

    /// Append one event to the audit log, creating the data directory and the
    /// log owner-only if needed. The append holds the store lock, so events
    /// from concurrent commands never interleave. An unsafe data directory is
    /// refused as for any write.
    pub fn record_audit(&self, event: &AuditEvent) -> Result<()> {
        let path = self.audit_path();
        let line = audit::encode_line(event)
            .map_err(|e| Error::io("encode", path.clone(), io::Error::other(e)))?;
        if !fsio::inspect_dir(&self.root)? {
            fsio::create_private_dir(&self.root)?;
        }
        let _lock = self.lock("audit")?;
        fsio::append_private(&path, &line)
    }

    fn lock(&self, label: &str) -> Result<LockGuard> {
        LockGuard::acquire(&self.root.join(LOCK_FILE), label, self.lock_wait)
    }

    fn root_exists(&self) -> Result<bool> {
        fsio::inspect_dir(&self.root)
    }

    /// Every entry, plus the files that did not parse. Anything other than
    /// a parse failure (a link, loose modes, a newer format, an I/O error)
    /// refuses the whole load.
    fn load_entries(&self) -> Result<(Vec<Entry>, Vec<Skipped>)> {
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

    /// Every `sessions/<yyyy>/<mm>/<id>.md`, checked but not yet parsed.
    fn session_files(&self) -> Result<Vec<SessionFile>> {
        let mut files = Vec::new();
        let dir = self.root.join(SESSIONS_DIR);
        if !self.root_exists()? || !fsio::inspect_subdir(&dir)? {
            return Ok(files);
        }
        let digits =
            |name: &str, len: usize| name.len() == len && name.bytes().all(|b| b.is_ascii_digit());
        for year in fsio::list_dir(&dir)?.into_iter().filter(|n| digits(n, 4)) {
            let year_dir = dir.join(&year);
            if !fsio::inspect_subdir(&year_dir)? {
                continue;
            }
            for month in fsio::list_dir(&year_dir)?
                .into_iter()
                .filter(|n| digits(n, 2))
            {
                let month_dir = year_dir.join(&month);
                if !fsio::inspect_subdir(&month_dir)? {
                    continue;
                }
                for name in visible(fsio::list_dir(&month_dir)?) {
                    let Some(stem) = name.strip_suffix(RECORD_EXT) else {
                        continue;
                    };
                    let path = month_dir.join(&name);
                    if fsio::inspect_file(&path)? {
                        files.push(SessionFile {
                            stem: stem.to_owned(),
                            year: year.clone(),
                            month: month.clone(),
                            path,
                        });
                    }
                }
            }
        }
        Ok(files)
    }
}

/// Append each of `more` not already in `links`.
fn merge_links(links: &mut Vec<SessionLink>, more: &[SessionLink]) {
    for link in more {
        if !links.contains(link) {
            links.push(link.clone());
        }
    }
}

/// Whether `entry` is from `session` and still waits for its message uuid.
fn unfilled(entry: &Entry, session: &SessionId) -> bool {
    &entry.cite.session == session && entry.cite.message.is_none()
}

/// Names that can be records: not hidden. A writer stages its temporary file
/// as a dot-file beside the target, and an editor leaves swap files the
/// same way; neither is a record.
fn visible(names: Vec<String>) -> impl Iterator<Item = String> {
    names.into_iter().filter(|n| !n.starts_with('.'))
}

/// A session record file found by the walk.
struct SessionFile {
    stem: String,
    year: String,
    month: String,
    path: PathBuf,
}

impl SessionFile {
    fn load(&self) -> Result<SessionRecord> {
        format::decode_session(
            &self.path,
            &fsio::read(&self.path)?,
            &self.stem,
            (&self.year, &self.month),
        )
    }
}

#[cfg(test)]
mod tests;
