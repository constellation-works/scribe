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
//! - `db`: `scribe.db`, the SQLite database of every record kind
//!   (2_design.md §15.7), the only user of `rusqlite`;
//! - `write`: a store write as one `scribe.db` transaction, the move of
//!   any record kind still in files, and audit lines that never wait;
//! - `documents`: entries, claims, tags, marks, pickups and the audit log,
//!   read from the database or, before the move, from their files;
//! - `fsio`: durable writes, locking and private-state checks, mechanism
//!   only.
//!
//! The layout (docs/design/scribe/2_design.md §3, §15.7):
//!
//! ```text
//! <root>/config.toml                        read-only to scribe
//! <root>/scribe.db                          every record kind (SQLite, WAL)
//! <root>/memory/                            profile.md, now/, milestones/
//! <root>/audit-spool/<time>-<pid>-<n>.json  audit lines a busy database could not take yet
//! <root>/control.json, proposals.json,      empty format stamps, so an older
//!   tags.json, marks.json, pickups.json       build refuses the store
//! <root>/pre-sqlite-<date>/                 the files the move took in, kept except on forget
//! <root>/pre-issue-<date>/<id>.md           issue records moved aside once converted
//! <root>/locks/                             advisory locks (Codex catalog, rebuild, archive forget)
//! ```
//!
//! Reads take no lock and write nothing (STD-01 R31): SQLite's WAL gives a
//! reader a committed snapshot, and the CLI works on a read-only root. Every
//! load refuses state that is a link, another user's, or open to any group or
//! other mode bit (STD-05 R7–R9), and a newer format (STD-03 R10). A record
//! that does not read is refused naming it; a listing skips it and reports it
//! in [`Skipped`], so one bad record does not hide the rest (4_decisions.md,
//! "One malformed record makes its file corrupt").

mod agent;
mod audit;
mod candidates;
mod continuation;
pub use continuation::HandoffSelection;
mod db;
pub use db::DB_FILE;
mod documents;
pub use documents::Moved;
mod format;
mod fsio;
mod ids;
mod init;
mod issue_import;
mod memory;
pub use memory::{
    MEMORY_DIR, MILESTONES_DIR, MemoryShown, MemoryView, MemoryWritten, NOW_DIR, PROFILE_FILE,
    PassMemory, ProfileInputs, ProfileRevision, RefreshWritten,
};
mod marks;
mod pickups;
pub use init::{InitFile, InitReport, init_project, integration_hooks};
mod rebuild;
mod redact;
mod sessions;
mod tags;
mod write;
pub use rebuild::RebuildGuard;
pub use sessions::{ExportFormat, Exported, LOCKS_DIR, SessionForget};

pub use agent::{check_agent_author, check_content, check_handoff_related, check_reason};
pub use audit::{AUDIT_FILE, AuditEvent, AuditStatus, IssueAudit};
pub use ids::resolve_prefix;
pub use marks::{COVERED, MODEL_FAILED, MarkChange, MarkKey, MarkResult, NEEDS_REDACTION};
pub use redact::{MIN_SECRET_CHARS, REDACTED_FIELDS, Redactor, unresolved_redact_env};
pub use tags::check_description;

use crate::activity::{self, ActiveOutcome, SetActive};
use crate::config::Config;
use crate::consistency::{self, Draft, ReapplyReport, Settle, Settled};
use crate::control;
use crate::entry::{Cite, Entry, EntryFilter, EntryId};
use crate::error::{Error, Result};
use crate::proposal::{Proposal, ProposalId};
use crate::record::Listing;
use crate::session::{
    self as session_mod, Child, ControlState, SessionFilter, SessionId, SessionLink, SessionRecord,
};
use crate::tag::{self, TagSummary};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
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
/// Consumers' pickups of action entries, from format 8.
pub const PICKUPS_FILE: &str = "pickups.json";
/// Each `#scribe` annotation marker's outcome, from format 22
/// (2_design.md §14.5).
pub const MARKS_FILE: &str = "marks.json";

/// A record file's extension.
const RECORD_EXT: &str = ".md";

/// How long a writer waits for another writer before refusing (STD-03 R7):
/// `scribe.db`'s `busy_timeout`, and the per-session locks' wait.
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
    /// Record files or rows left out because they did not parse.
    pub skipped: Vec<Skipped>,
    /// The session ids of the skipped records, where that much reads.
    pub skipped_sessions: Vec<SessionId>,
    /// Every session's `s-<n>`.
    pub numbers: BTreeMap<SessionId, NonZeroU64>,
    /// When each forgotten session was forgotten.
    pub forgotten: BTreeMap<SessionId, time::OffsetDateTime>,
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
    /// Its `s-<n>`.
    pub number: Option<NonZeroU64>,
    /// When `#scribe forget` erased it, if it did.
    pub forgotten: Option<time::OffsetDateTime>,
}

/// What [`Store::list_tags`] found.
#[derive(Debug)]
pub struct TagList {
    /// Every configured tag and every tag with entries, by name.
    pub tags: Vec<TagSummary>,
    /// Config overrides reported to the caller.
    pub warnings: Vec<String>,
    /// Entry files left out of the counts because they did not parse.
    pub skipped: Vec<Skipped>,
}

/// A scribe store rooted at one data directory.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
    lock_wait: Duration,
}

/// How [`Store::write_pass_window`] leaves the switch.
#[derive(Clone, Copy, Debug)]
pub enum PassWrite {
    /// An intermediate window. Force the session due.
    Checkpoint,
    /// The last window of an ordinary pass. `bool` is `ran_final`.
    Settle(bool),
    /// The last window of a live catalog: stay on and due, clear backoff.
    CatalogLive,
    /// The last window of an ended catalog: settle as final and switch off.
    CatalogFinal,
}

/// What [`Store::write_pass_window`] wrote under the store lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassWritten {
    /// The updated session record, when one was written.
    pub record: Option<SessionRecord>,
    /// Open candidate proposals closed because their tag is absent from the
    /// tag table, in id order.
    pub closed_candidates: Vec<Proposal>,
    /// Entries the pass wrote itself under `write` tags (2_design.md
    /// §11.6), in write order.
    pub written_entries: Vec<EntryId>,
    /// The window's memory write (2_design.md §13.3).
    pub memory: MemoryWritten,
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
        let mut config = self.config_only()?;
        config.tags = self.merge_tags(&config.tags)?.0;
        Ok(config)
    }

    pub(super) fn config_only(&self) -> Result<Config> {
        let path = self.root.join(CONFIG_FILE);
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(Config::default());
        }
        let bytes = fsio::read(&path)?;
        let text = std::str::from_utf8(&bytes).map_err(|e| Error::corrupt(&path, e))?;
        let config = Config::parse(text).map_err(|source| Error::Config { path, source })?;
        // Keys this build no longer reads: logged once per process, however
        // often a command loads the config.
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !config.warnings.is_empty() && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            for warning in &config.warnings {
                tracing::warn!("{warning}");
            }
        }
        Ok(config)
    }

    /// Active `handoff` entries referencing `session:<id>`, newest first.
    /// Reads only: no lock, no write.
    pub fn handoffs_for_session(&self, id: &SessionId) -> Result<Vec<Entry>> {
        let (entries, _) = self.load_entries()?;
        Ok(crate::handoff::for_session(&entries, id)
            .into_iter()
            .cloned()
            .collect())
    }

    /// Entries matching `filter`, oldest first, and how many matched. A
    /// missing store is an empty one.
    pub fn list_entries(&self, filter: &EntryFilter) -> Result<EntryList> {
        self.list_entries_scoped(filter, None)
    }

    /// List entries, optionally restricted to the group's member sessions
    /// before applying the filter and its limit.
    pub fn list_entries_scoped(
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

    /// Session records matching `filter`, oldest first, with the on/off
    /// state beside them.
    pub fn list_sessions(&self, filter: &SessionFilter) -> Result<SessionList> {
        self.list_sessions_scoped(filter, None)
    }

    /// List session records, optionally restricting the records and their
    /// derived lookup data to the group's member sessions.
    pub fn list_sessions_scoped(
        &self,
        filter: &SessionFilter,
        members: Option<&BTreeSet<SessionId>>,
    ) -> Result<SessionList> {
        let mut loaded = self.load_sessions()?;
        if let Some(members) = members {
            loaded.records.retain(|r| members.contains(&r.id));
            loaded
                .skipped
                .retain(|(id, _)| id.as_ref().is_some_and(|id| members.contains(id)));
        }
        let control = loaded.control;
        let records = loaded.records;
        let skipped_sessions = loaded
            .skipped
            .iter()
            .filter_map(|(id, _)| id.clone())
            .collect();
        let skipped: Vec<Skipped> = loaded.skipped.into_iter().map(|(_, s)| s).collect();
        let numbers = loaded.numbers;
        let forgotten = loaded.forgotten;
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
            skipped_sessions,
            numbers,
            forgotten,
        })
    }

    /// Refuse `id` when `#scribe forget` erased it, before any model call or
    /// write on its behalf: a forgotten session takes no write (`session_forgotten`).
    /// Reads only.
    ///
    /// # Errors
    ///
    /// `session_forgotten` for a forgotten session, or a store read error.
    pub fn refuse_forgotten(&self, id: &SessionId) -> Result<()> {
        if self.load_sessions()?.forgotten.contains_key(id) {
            return Err(db::sessions::forgotten_refusal(
                id,
                "nothing is written to it",
            ));
        }
        Ok(())
    }

    /// The session record with this id, its switch, the sessions that link
    /// to it and the titles of those it links to. Other records that do not
    /// parse are left out of the children.
    pub fn session(&self, id: &SessionId) -> Result<SessionView> {
        let mut loaded = self.load_sessions()?;
        let record = match loaded.take(id) {
            Some(record) => record?,
            None => {
                return Err(Error::SessionNotFound {
                    id: id.clone(),
                    path: self.sessions_place()?,
                });
            }
        };
        let control = loaded.control;
        let number = loaded.numbers.get(id).copied();
        let forgotten = loaded.forgotten.get(id).copied();
        let mut others = loaded.records;
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
            number,
            forgotten,
        })
    }

    /// Where the sessions are, for a not-found error: `scribe.db` once they
    /// moved there, else `sessions/`.
    fn sessions_place(&self) -> Result<PathBuf> {
        Ok(
            if self.root_exists()? && db::sessions::read_moved(&self.root)? {
                self.root.join(DB_FILE)
            } else {
                self.root.join(SESSIONS_DIR)
            },
        )
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
        self.link_session_for_provider(session, link, on, now, crate::Provider::ClaudeCode)
    }

    /// Link and stamp the origin provider under the same store lock.
    pub fn link_session_for_provider(
        &self,
        session: &SessionId,
        link: &SessionLink,
        on: bool,
        now: time::OffsetDateTime,
        provider: crate::Provider,
    ) -> Result<control::Linked> {
        self.write(|| {
            self.apply_session_link(
                session,
                link,
                control::LinkSwitch::Manual { on },
                now,
                provider,
            )
        })
    }

    fn apply_session_link(
        &self,
        session: &SessionId,
        link: &SessionLink,
        mode: control::LinkSwitch,
        now: time::OffsetDateTime,
        provider: crate::Provider,
    ) -> Result<control::Linked> {
        let loaded = self.load_sessions()?;
        if matches!(mode, control::LinkSwitch::Inherit) {
            for id in [session, &link.session] {
                if loaded.forgotten.contains_key(id) {
                    return Err(db::sessions::forgotten_refusal(
                        id,
                        "it cannot be continued",
                    ));
                }
                if loaded
                    .control
                    .get(id)
                    .is_some_and(|c| c.provider != provider)
                {
                    return Err(agent::refusal(
                        crate::WriteRefusal::HandoffSession,
                        "session belongs to another transcript provider",
                    ));
                }
            }
        }
        let mut own: Option<SessionRecord> = None;
        let mut graph: BTreeMap<SessionId, Vec<SessionLink>> = BTreeMap::new();
        for record in loaded.records {
            graph.insert(record.id.clone(), record.links.clone());
            if &record.id == session {
                own = Some(record);
            }
        }
        for (id, bad) in loaded.skipped {
            match id {
                Some(id) if &id == session => return Err(bad.error),
                // Known by id, its links unreadable.
                Some(id) => {
                    graph.entry(id).or_default();
                }
                None => {}
            }
        }
        // A refused link changes nothing, so nothing is written.
        self.sessions_tx(|tx| {
            let mut state = tx.control()?;
            let before = state.clone();
            let linked = control::apply_link(&mut state, &graph, session, link, mode, now)?;
            if let Some(control) = state.sessions.get_mut(session) {
                control.provider = provider;
            }
            if state != before {
                tx.put_control(&before, &state)?;
            }
            if linked.write_record
                && let Some(mut record) = own
            {
                record.links.push(link.clone());
                tx.put_record(&record)?;
            }
            Ok(linked)
        })
    }

    /// Change one session's `active` or `keep_active` under the store lock
    /// ([`activity::set_active`]), its reason masked by `redactor`. Only
    /// `active`, `keep_active` and `active_change` change (`updated`
    /// included stays), and the record is written only when something
    /// changed. Returns the record as it now stands and what changed.
    ///
    /// # Errors
    ///
    /// [`Error::SessionNotFound`] when no record has the id; a store error
    /// otherwise.
    pub fn set_active(
        &self,
        id: &SessionId,
        request: &SetActive,
        now: time::OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(SessionRecord, ActiveOutcome)> {
        check_active_author(request)?;
        self.prepare_dir(&self.root)?;
        self.write(|| {
            let mut loaded = self.load_sessions()?;
            let mut record = match loaded.take(id) {
                Some(record) => record?,
                None => {
                    return Err(Error::SessionNotFound {
                        id: id.clone(),
                        path: self.sessions_place()?,
                    });
                }
            };
            let outcome = self.apply_active(&mut record, request, now, redactor)?;
            Ok((record, outcome))
        })
    }

    /// Apply the routine's deactivations under one store lock, each
    /// checked again against the record on disk (a session pinned or
    /// already deactivated since the scan read it is left alone). A
    /// missing or unreadable record fails only its own request; the other
    /// requests continue. Returns each outcome in request order.
    pub fn set_active_many(
        &self,
        requests: &[(SessionId, SetActive)],
        now: time::OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Vec<Result<ActiveOutcome>>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        self.prepare_dir(&self.root)?;
        self.write(|| {
            let loaded = self.load_sessions()?;
            let mut out = Vec::with_capacity(requests.len());
            for (id, request) in requests {
                let mut record = match loaded.records.iter().find(|r| &r.id == id) {
                    Some(record) => record.clone(),
                    None => {
                        out.push(Err(
                            match loaded.skipped.iter().find(|(s, _)| s.as_ref() == Some(id)) {
                                Some((_, bad)) => {
                                    Error::corrupt(bad.path.clone(), bad.error.to_string())
                                }
                                None => Error::SessionNotFound {
                                    id: id.clone(),
                                    path: self.sessions_place()?,
                                },
                            },
                        ));
                        continue;
                    }
                };
                out.push(self.apply_active(&mut record, request, now, redactor));
            }
            Ok(out)
        })
    }

    /// Apply `request` to `record` and write it back when it changed. The
    /// caller holds the lock.
    fn apply_active(
        &self,
        record: &mut SessionRecord,
        request: &SetActive,
        now: time::OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<ActiveOutcome> {
        check_active_author(request)?;
        let outcome = activity::set_active(record, request, now);
        if outcome.changed() {
            if let Some(change) = record.active_change.as_mut() {
                change.reason = redactor.mask(&change.reason);
                if let Some(cite) = change.cite.as_mut() {
                    *cite = redactor.mask(cite);
                }
            }
            self.sessions_tx(|tx| tx.put_record(record).map(|_| ()))?;
        }
        Ok(outcome)
    }

    /// The on/off state and Codex cursors: from `scribe.db` once the
    /// sessions moved there, else `control.json`; empty when neither has
    /// any.
    pub fn control(&self) -> Result<ControlState> {
        if !self.root_exists()? {
            return Ok(ControlState::default());
        }
        match db::sessions::read_control(&self.root)? {
            Some(state) => self.check_stamp().map(|()| state),
            None => self.control_file(),
        }
    }

    /// Every configured tag and every tag that has entries, with counts.
    pub fn list_tags(&self) -> Result<TagList> {
        let config = self.config_only()?;
        let stored = self.store_tags()?;
        let (table, mut warnings) = config
            .tags
            .merge_store(&stored)
            .map_err(|e| agent::refusal(crate::WriteRefusal::TagConflict, e.to_string()))?;
        warnings.extend(config.tag_warnings);
        let configured = &config.tags;
        let (entries, skipped) = self.load_entries()?;
        let pickups = self.pickups()?;
        let counts = crate::action::tag_counts(&entries, &table, &pickups);
        let mut tags = tag::summarize(&table, &entries);
        for tag in &mut tags {
            if let Some((retired, rejected)) = counts.get(&tag.name) {
                tag.retired = *retired;
                tag.rejected = *rejected;
            }
            tag.source = if configured.get(&tag.name).is_some() {
                Some("config")
            } else if table.get(&tag.name).is_some() {
                Some("store")
            } else {
                None
            };
            if tag.source == Some("store") {
                tag.author = stored
                    .iter()
                    .find(|t| t.name == tag.name)
                    .map(|t| t.author.clone());
            }
        }
        Ok(TagList {
            tags,
            skipped,
            warnings,
        })
    }

    /// The entries already recorded from one prompt: `session` and
    /// `prompt_sha256` as their cite has them. Reads only, so `Stop` can
    /// skip a `#scribe` line an entry was already written for.
    pub fn entries_from_prompt(
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
        self.write(|| {
            let (mut entries, _) = self.load_entries()?;
            entries.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
            let mut filled = Vec::new();
            for mut entry in entries.into_iter().filter(|e| unfilled(e, session)) {
                let Some(cite) = entry.cite.as_mut() else {
                    continue;
                };
                let Some(message) = find(cite) else {
                    continue;
                };
                cite.message = Some(message);
                entry.updated = now;
                self.write_entry(&entry, false)?;
                filled.push(entry.id);
            }
            Ok(filled)
        })
    }

    /// Change the on/off state under the store lock: `change` gets the
    /// current state and its result is returned. `control.json` is
    /// rewritten only when the state changed. The data directory is created
    /// owner-only when absent, so a caller that may change nothing (the
    /// `Stop` hook) reads [`Store::control`] first and calls this only when
    /// a change is likely.
    pub fn update_control<T>(&self, change: impl FnOnce(&mut ControlState) -> T) -> Result<T> {
        self.prepare_dir(&self.root)?;
        self.write(|| self.change_control(change))
    }

    /// [`Store::update_control`] for a caller that holds the lock.
    /// Nothing is written when nothing changed: the lock keeps the state
    /// read here current.
    fn change_control<T>(&self, change: impl FnOnce(&mut ControlState) -> T) -> Result<T> {
        let mut state = self.control()?;
        let before = state.clone();
        let out = change(&mut state);
        if state != before {
            self.sessions_tx(|tx| tx.put_control(&before, &state))?;
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
    ///
    /// `expected` is the `updated` revision the pass loaded (`None` when it
    /// loaded no record). Under the lock it is compared with the record on
    /// disk before anything is written; a mismatch refuses the write with
    /// [`Error::RecordChanged`], leaving the record on disk unchanged and
    /// the session due, so the next run starts from that record. This
    /// mirrors [`Store::finish_rebuild`].
    pub fn settle_pass(
        &self,
        session: &SessionId,
        record: Option<SessionRecord>,
        ran_final: bool,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        self.write_pass(
            session,
            record,
            PassWrite::Settle(ran_final),
            expected,
            redactor,
        )
    }

    /// Persist one intermediate window under the store lock, redacted and
    /// atomically replaced. Keep the pass due until its last window settles.
    /// `expected` is checked as in [`Store::settle_pass`].
    pub fn checkpoint_pass(
        &self,
        record: SessionRecord,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        let session = record.id.clone();
        self.write_pass(
            &session,
            Some(record),
            PassWrite::Checkpoint,
            expected,
            redactor,
        )
    }

    /// The last window of a live `session catalog`: write the record and
    /// leave the switch on and due, clearing retry backoff, so the hooks
    /// carry on. `expected` is checked as in [`Store::settle_pass`].
    pub fn checkpoint_catalog(
        &self,
        record: SessionRecord,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        let session = record.id.clone();
        self.write_pass(
            &session,
            Some(record),
            PassWrite::CatalogLive,
            expected,
            redactor,
        )
    }

    /// The last window of an ended `session catalog`, under one lock: write
    /// the record, settle the pass as final, and switch the session off so
    /// no hook passes it again. A crash cannot leave it on but not due.
    pub fn settle_catalog_final(
        &self,
        session: &SessionId,
        record: Option<SessionRecord>,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        self.write_pass(session, record, PassWrite::CatalogFinal, expected, redactor)
    }

    /// Write a checked pass window and its candidates under the same lock.
    /// Candidates are persisted first, so an interruption retries safely.
    /// Then one `scribe.db` transaction commits the record (its cursor
    /// advanced), the window's `messages`, the memory rows and journal and
    /// the switch; the memory files follow the journal (2_design.md §15.6,
    /// §15.7). Retired-tag open candidates are closed before caps are
    /// counted, duplicates and each claim's trigger (a trigger of its tag
    /// fires in its turn, under the config read under the lock) are checked
    /// here.
    #[allow(clippy::too_many_arguments)]
    pub fn write_pass_window(
        &self,
        record: SessionRecord,
        expected: Option<time::OffsetDateTime>,
        kind: PassWrite,
        candidates: &[crate::pass::PassClaim],
        memory: Option<&PassMemory>,
        messages: &[crate::message::NewMessage],
        redactor: &Redactor,
    ) -> Result<PassWritten> {
        let session = record.id.clone();
        self.write_pass_candidates(
            &session,
            Some(record),
            kind,
            expected,
            PassParts {
                candidates,
                memory,
                messages,
                run_candidates: true,
            },
            redactor,
        )
    }

    fn write_pass(
        &self,
        session: &SessionId,
        record: Option<SessionRecord>,
        kind: PassWrite,
        expected: Option<time::OffsetDateTime>,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        Ok(self
            .write_pass_candidates(
                session,
                record,
                kind,
                expected,
                PassParts {
                    candidates: &[],
                    memory: None,
                    messages: &[],
                    run_candidates: false,
                },
                redactor,
            )?
            .record)
    }

    fn write_pass_candidates(
        &self,
        session: &SessionId,
        record: Option<SessionRecord>,
        kind: PassWrite,
        expected: Option<time::OffsetDateTime>,
        parts: PassParts<'_>,
        redactor: &Redactor,
    ) -> Result<PassWritten> {
        self.prepare_dir(&self.root)?;
        self.write(|| {
            // Compare the revision the pass loaded with the stored one before
            // any write, so a rebuild that finished while the model ran is
            // never overwritten by this stale pass. A link keeps `updated`, so
            // a linked record still matches and its links are merged below.
            let mut loaded = self.load_sessions()?;
            let current = loaded.take(session).transpose()?;
            if current.as_ref().map(|c| c.updated) != expected {
                return Err(Error::RecordChanged {
                    id: session.clone(),
                });
            }
            let pending = loaded
                .control
                .get(session)
                .map(|c| c.pending_links.clone())
                .unwrap_or_default();
            let mut record = record;
            let mut claims = candidates::ClaimsWritten::default();
            if let Some(record) = record.as_mut() {
                // A link written while the model ran, or one waiting on the
                // switch for the first record, is kept: never lost to a pass.
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
                merge_links(&mut record.links, &pending);
                redactor.session(record);
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
                if parts.run_candidates {
                    claims = self.open_pass_candidates(record, parts.candidates, redactor)?;
                }
            }
            // The closures, written entries, record, messages, memory rows and
            // journal and the switch all commit in this one transaction, so a
            // later failure leaves none of them.
            let conn = db::open_writer(&self.root, self.lock_wait)?;
            // Memory is planned first (any open journal replayed, `m-<n>` items
            // migrated); its rows and journal commit with the record below.
            let (memory_commit, memory_written) = match (parts.memory, &record) {
                (Some(memory), Some(record)) => {
                    let mut writer = db::Writer::new(&conn, &self.root);
                    self.plan_pass_memory(&mut writer, memory, redactor, record)?
                }
                _ => (db::MemoryCommit::default(), MemoryWritten::default()),
            };
            let tx = db::sessions::SessionTx::begin(&conn, &self.root)?;
            let mut state = tx.control()?;
            let before = state.clone();
            // An explicit pass can start when not due. Publish the obligation
            // with the checkpoint, so interruption cannot strand its remainder.
            // A live catalog also clears retry backoff: the hooks take it from
            // here.
            if matches!(kind, PassWrite::Checkpoint | PassWrite::CatalogLive)
                && let Some(control) = state.sessions.get_mut(session).filter(|c| c.on)
            {
                control.due = true;
                if matches!(kind, PassWrite::CatalogLive) {
                    control.consecutive_failures = 0;
                    control.retry_after = None;
                }
            }
            if let Some(record) = &record {
                tx.put_record(record)?;
                tx.upsert_messages(parts.messages, redactor)?;
                if !memory_commit.is_empty() {
                    // A pass forgets nothing, so it erases no message. The
                    // files follow the journal after the commit.
                    if tx.stage_memory(&memory_commit, record.updated)?.0.is_some() {
                        self.defer_journals();
                    }
                }
                // Only once the record holds them: the links are stored once.
                if !pending.is_empty()
                    && let Some(control) = state.sessions.get_mut(session)
                {
                    control.pending_links.retain(|l| !pending.contains(l));
                }
            }
            match kind {
                PassWrite::Settle(ran_final) => {
                    control::settle(&mut state, session, ran_final);
                }
                PassWrite::CatalogFinal => {
                    // Settle and switch off together. Off does not record a time.
                    control::settle(&mut state, session, true);
                    let _ = control::switch(
                        &mut state,
                        session,
                        &control::Switch::Off,
                        time::OffsetDateTime::UNIX_EPOCH,
                    );
                }
                PassWrite::Checkpoint | PassWrite::CatalogLive => {}
            }
            if state != before {
                tx.put_control(&before, &state)?;
            }
            tx.commit()?;
            Ok(PassWritten {
                record,
                closed_candidates: claims.closed,
                written_entries: claims.written,
                memory: memory_written,
            })
        })
    }

    /// Re-apply, under the store lock, every approved proposal whose older
    /// entry is still `active` ([`consistency::reapply`]), and create any
    /// missing entry from an approved candidate. An approval
    /// writes `proposals.json` and then the entry, and this repairs a crash
    /// between the two. Writes only the entries it changes. An approved
    /// candidate an active entry of the same tag and project already covers
    /// is closed as `rejected` instead, with no new entry; that close is
    /// [`ReapplyReport::covered`] and the covering id is not stored on the
    /// proposal.
    pub fn reapply_approved(&self, now: time::OffsetDateTime) -> Result<ReapplyReport> {
        if !self.root_exists()? {
            return Ok(ReapplyReport::default());
        }
        let mut report = self.reapply_candidates()?;
        let proposals = self.proposals()?;
        let (entries, _) = self.load_entries()?;
        let mut probe = entries;
        if consistency::reapply(&proposals, &mut probe, now).is_empty() {
            return Ok(report);
        }
        self.write(|| {
            let proposals = self.proposals()?;
            let (mut entries, _) = self.load_entries()?;
            let done = consistency::reapply(&proposals, &mut entries, now);
            for change in &done {
                if let Some(entry) = entries.iter().find(|e| e.id == change.entry) {
                    self.write_entry(entry, false)?;
                }
            }
            report.applied.extend(done);
            Ok(report)
        })
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
        self.write(|| {
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
        })
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
        self.write(|| {
            if let Settle::ForgetSession(id) = action {
                return Ok(match self.forget_session_locked(id, now)? {
                    sessions::SessionForgetOutcome::Forgotten(forgot) => {
                        Settled::SessionForgotten(forgot)
                    }
                    sessions::SessionForgetOutcome::Already {
                        id: resolved,
                        number,
                        cleaned: true,
                    } => Settled::SessionForgetRepaired {
                        id: resolved,
                        number,
                    },
                    sessions::SessionForgetOutcome::Already { cleaned: false, .. } => {
                        Settled::Already(format!("{id} is already forgotten"))
                    }
                });
            }
            let mut proposals = self.proposals()?;
            let (mut entries, _) = self.load_entries()?;
            let before_proposals = proposals.clone();
            let before_entries = entries.clone();
            let mut settled = consistency::settle(action, &mut proposals, &mut entries, cite, now);
            if let (Settle::Forget(id), Settled::Forgotten(_)) = (action, &settled)
                && let Some(entry) = before_entries.iter().find(|e| &e.id == id)
            {
                // The stored messages first (2_design.md §15.7): they hold the
                // words the forget removes, and deleting them again is a no-op;
                // moved `pre-sqlite-*` copies follow the commit via a durable
                // obligation.
                self.forget_entry_residue(entry, &before_proposals, now)?;
            }
            if proposals != before_proposals {
                self.write_proposals(&proposals)?;
            }
            if let Settle::Approve(id) = action
                && let Some(index) = proposals.iter().position(|p| {
                    p.id == *id
                        && p.kind == crate::ProposalKind::Candidate
                        && p.state == crate::ProposalState::Approved
                })
            {
                let proposal = proposals[index].clone();
                match self.write_candidate_entry(&proposal)? {
                    candidates::CandidateWrite::Created => {
                        settled = Settled::Recorded {
                            proposal: *id,
                            entry: proposal.older.clone(),
                            agent_authored: proposal
                                .candidate
                                .as_ref()
                                .is_some_and(|c| c.role == crate::Speaker::Assistant),
                        };
                    }
                    candidates::CandidateWrite::Present => {}
                    candidates::CandidateWrite::Covered { by } => {
                        // Approved, but an active entry now says it: closed as
                        // covered, keeping who approved it and on what
                        // instruction, never as a rejection (format 15).
                        proposals[index].state = crate::ProposalState::Rejected;
                        proposals[index].closed_reason = Some(crate::ClosedReason::Covered);
                        self.write_proposals(&proposals)?;
                        settled = Settled::Covered { proposal: *id, by };
                    }
                }
            }
            for (entry, before) in entries.iter().zip(&before_entries) {
                if entry != before {
                    self.write_entry(entry, false)?;
                }
            }
            Ok(settled)
        })
    }

    /// Keep a pass window's messages in `scribe.db` (2_design.md §15.7),
    /// each text redacted first, upserted on (session, message id) in one
    /// `BEGIN IMMEDIATE` transaction. The pass calls this before it writes
    /// the session file, whose cursor stays the truth: a window written
    /// again rewrites the same rows. Returns how many rows changed.
    ///
    /// # Errors
    ///
    /// [`Error::Locked`] when another writer holds the database past the
    /// lock wait; the privacy errors when the root or the database is a
    /// link, another user's or open to others; [`Error::NewerFormat`] for a
    /// database a newer build wrote.
    pub fn write_messages(
        &self,
        messages: &[crate::message::NewMessage],
        redactor: &Redactor,
    ) -> Result<usize> {
        if messages.is_empty() {
            return Ok(0);
        }
        self.prepare_dir(&self.root)?;
        db::upsert_messages(&self.root, self.lock_wait, messages, redactor)
    }

    /// Delete stored messages by (session, message id), for a forget,
    /// leaving no trace of their text in the database or its WAL. A missing
    /// database deletes nothing.
    fn forget_messages(&self, keys: &[(SessionId, String)]) -> Result<usize> {
        db::delete_messages(&self.root, self.lock_wait, keys)
    }

    /// Delete the stored messages a forgotten `entry` rests on: the turn it
    /// cites and the turns its candidate proposals cite.
    fn forget_cited_messages(&self, entry: &Entry, proposals: &[Proposal]) -> Result<usize> {
        let keys: Vec<_> = entry
            .cite
            .iter()
            .chain(
                proposals
                    .iter()
                    .filter(|p| p.older == entry.id)
                    .filter_map(|p| p.candidate.as_ref().map(|c| &c.cite)),
            )
            .filter_map(|c| c.message.clone().map(|m| (c.session.clone(), m)))
            .collect();
        self.forget_messages(&keys)
    }

    /// The stored messages matching any of `tokens` (ranking tokens, as
    /// [`crate::ask::tokens`] gives them) as a word prefix, best FTS5
    /// `bm25()` first. Read-only: no lock, no write (STD-01 R31). A missing
    /// root or database matches nothing.
    ///
    /// # Errors
    ///
    /// The privacy errors for the root or the database, [`Error::NewerFormat`]
    /// and [`Error::Database`].
    pub fn search_messages(&self, tokens: &[String]) -> Result<crate::message::MessageSearch> {
        if !self.root_exists()? {
            return Ok(crate::message::MessageSearch::default());
        }
        db::search_messages(&self.root, tokens, None)
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

    fn root_exists(&self) -> Result<bool> {
        fsio::inspect_dir(&self.root)
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

/// The parts of a pass window besides its record.
struct PassParts<'a> {
    /// Claims to open.
    candidates: &'a [crate::pass::PassClaim],
    /// The window's memory, planned and committed with the record.
    memory: Option<&'a PassMemory>,
    /// The window's messages, upserted with the record.
    messages: &'a [crate::message::NewMessage],
    /// Whether to open `candidates` (and close retired-tag ones).
    run_candidates: bool,
}

/// The routine's own routes carry a scribe author; an agent author here
/// is a CLI write verb, which may not use a reserved name.
fn check_active_author(request: &SetActive) -> Result<()> {
    let by = match request {
        SetActive::Deactivate { by, .. } | SetActive::Activate { by, .. } => by,
    };
    if by.kind == crate::AuthorKind::Agent {
        agent::check_agent_author(&by.name)?;
    }
    Ok(())
}

fn merge_links(links: &mut Vec<SessionLink>, more: &[SessionLink]) {
    for link in more {
        if !links.contains(link) {
            links.push(link.clone());
        }
    }
}

/// Whether `entry` is from `session` and still waits for its message uuid.
fn unfilled(entry: &Entry, session: &SessionId) -> bool {
    entry
        .cite
        .as_ref()
        .is_some_and(|c| &c.session == session && c.message.is_none())
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
