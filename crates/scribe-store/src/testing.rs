//! Test support: a shim that keeps the tests' call syntax
//! (`store.session(id)`, `store.control()`, …) after ORB-27707 step 2c-4c
//! narrowed `Store` to its lifecycle, so their bodies stay as they were.
//! It is not API: production code reads and writes through
//! [`Store::read`] and [`Store::write`]. Compiled for tests and the
//! `test-support` feature only.
//!
//! It also holds what the store's tests share across the two crates
//! (ORB-27707 step 4b): the helpers ([`private_tempdir`], [`seed`], …) and,
//! in the modules named after the store's own, the store internals that
//! scribe-core's store tests reach. Nothing here is in a normal build's
//! public API.
#![allow(clippy::unwrap_used)]

use crate::group::Group;
use crate::message::{MessageSearch, NewMessage};
use crate::{
    AuditEvent, Config, ControlState, Entry, EntryFilter, EntryId, EntryList, Proposal, Redactor,
    Result, SessionFilter, SessionId, SessionList, SessionView, Store, TagList,
};
use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::path::PathBuf;

/// The store reads and writes the tests call by their pre-2c-4c names.
/// Each is one delegation to a repository.
pub trait StoreTestReads {
    /// [`crate::repository::Sessions::get`].
    fn session(&self, id: &SessionId) -> Result<SessionView>;
    /// [`crate::repository::Sessions::control`].
    fn control(&self) -> Result<ControlState>;
    /// [`crate::repository::Sessions::change_control`] in a [`Store::write`].
    fn update_control<T>(&self, change: impl FnOnce(&mut ControlState) -> T) -> Result<T>;
    /// [`crate::repository::Sessions::list`].
    fn list_sessions(&self, filter: &SessionFilter) -> Result<SessionList>;
    /// [`crate::repository::Sessions::numbers`].
    fn session_numbers(&self) -> Result<BTreeMap<SessionId, NonZeroU64>>;
    /// [`crate::repository::Entries::get`].
    fn entry(&self, id: &EntryId) -> Result<Entry>;
    /// [`crate::repository::Entries::list`].
    fn list_entries(&self, filter: &EntryFilter) -> Result<EntryList>;
    /// [`crate::repository::Proposals::all`].
    fn proposals(&self) -> Result<Vec<Proposal>>;
    /// [`crate::repository::Tags::config`].
    fn config(&self) -> Result<Config>;
    /// [`crate::repository::Tags::list`].
    fn list_tags(&self) -> Result<TagList>;
    /// [`crate::repository::Groups::all`].
    fn groups(&self) -> Result<Vec<Group>>;
    /// [`crate::repository::Groups::get`].
    fn group(&self, id: &str) -> Result<Group>;
    /// [`crate::repository::Groups::search_messages`].
    fn search_group_messages(&self, group: &str, tokens: &[String]) -> Result<MessageSearch>;
    /// [`crate::repository::Messages::search`].
    fn search_messages(&self, tokens: &[String]) -> Result<MessageSearch>;
    /// [`crate::repository::Messages::put`] in a [`Store::write`].
    fn write_messages(&self, messages: &[NewMessage], redactor: &Redactor) -> Result<usize>;
    /// [`crate::repository::Audit::events`].
    fn audit_events(&self) -> Result<(Vec<AuditEvent>, usize)>;
    /// [`crate::repository::Audit::path`].
    fn audit_path(&self) -> PathBuf;
}

impl StoreTestReads for Store {
    fn session(&self, id: &SessionId) -> Result<SessionView> {
        self.read().sessions().get(id)
    }
    fn control(&self) -> Result<ControlState> {
        self.read().sessions().control()
    }
    fn update_control<T>(&self, change: impl FnOnce(&mut ControlState) -> T) -> Result<T> {
        self.write(|tx| tx.sessions().change_control(change))
    }
    fn list_sessions(&self, filter: &SessionFilter) -> Result<SessionList> {
        self.read().sessions().list(filter)
    }
    fn session_numbers(&self) -> Result<BTreeMap<SessionId, NonZeroU64>> {
        self.read().sessions().numbers()
    }
    fn entry(&self, id: &EntryId) -> Result<Entry> {
        self.read().entries().get(id)
    }
    fn list_entries(&self, filter: &EntryFilter) -> Result<EntryList> {
        self.read().entries().list(filter)
    }
    fn proposals(&self) -> Result<Vec<Proposal>> {
        self.read().proposals().all()
    }
    fn config(&self) -> Result<Config> {
        self.read().tags().config()
    }
    fn list_tags(&self) -> Result<TagList> {
        self.read().tags().list()
    }
    fn groups(&self) -> Result<Vec<Group>> {
        self.read().groups().all()
    }
    fn group(&self, id: &str) -> Result<Group> {
        self.read().groups().get(id)
    }
    fn search_group_messages(&self, group: &str, tokens: &[String]) -> Result<MessageSearch> {
        self.read().groups().search_messages(group, tokens)
    }
    fn search_messages(&self, tokens: &[String]) -> Result<MessageSearch> {
        self.read().messages().search(tokens)
    }
    fn write_messages(&self, messages: &[NewMessage], redactor: &Redactor) -> Result<usize> {
        self.write(|tx| tx.messages().put(messages, redactor))
    }
    fn audit_events(&self) -> Result<(Vec<AuditEvent>, usize)> {
        self.read().audit().events()
    }
    fn audit_path(&self) -> PathBuf {
        self.read().audit().path()
    }
}

/// The store's own methods its tests in scribe-core call, by their names:
/// each is one delegation to the store-private method, so those tests run
/// the same code they did inside the store. Only for tests.
pub trait StoreTestInternals {
    /// How long a write waits for the store lock (`Store::lock_wait`).
    fn lock_wait(&self) -> std::time::Duration;
    /// Group `id`. (`Store::group`).
    fn group(&self, id: &str) -> Result<crate::group::Group>;
    /// Search `group`'s stored messages. (`Store::search_group_messages`).
    fn search_group_messages(
        &self,
        group: &str,
        tokens: &[String],
    ) -> Result<crate::message::MessageSearch>;
    /// Entry `id`. (`Store::entry`).
    fn entry(&self, id: &crate::EntryId) -> Result<crate::Entry>;
    /// Every claim. (`Store::proposals`).
    fn proposals(&self) -> Result<Vec<crate::Proposal>>;
    /// The audit log, and how many lines did not read. (`Store::audit_events`).
    fn audit_events(&self) -> Result<(Vec<crate::AuditEvent>, usize)>;
    /// The configuration. (`Store::config`).
    fn config(&self) -> Result<crate::Config>;
    /// The entries `filter` selects. (`Store::list_entries`).
    fn list_entries(&self, filter: &crate::EntryFilter) -> Result<crate::EntryList>;
    /// The sessions `filter` selects. (`Store::list_sessions`).
    fn list_sessions(&self, filter: &crate::SessionFilter) -> Result<crate::SessionList>;
    /// Session `id`. (`Store::session`).
    fn session(&self, id: &crate::SessionId) -> Result<crate::SessionView>;
    /// The on/off state. (`Store::control`).
    fn control(&self) -> Result<crate::ControlState>;
    /// The tag table. (`Store::list_tags`).
    fn list_tags(&self) -> Result<crate::TagList>;
    /// Store typed turns. (`Store::write_messages`).
    fn write_messages(
        &self,
        messages: &[crate::message::NewMessage],
        redactor: &crate::Redactor,
    ) -> Result<usize>;
    /// Search the stored messages. (`Store::search_messages`).
    fn search_messages(&self, tokens: &[String]) -> Result<crate::message::MessageSearch>;
    /// Every session's number. (`Store::session_numbers`).
    fn session_numbers(
        &self,
    ) -> Result<std::collections::BTreeMap<crate::SessionId, std::num::NonZeroU64>>;
    /// Change the on/off state under the store lock (`Store::update_control`).
    fn update_control<T>(&self, change: impl FnOnce(&mut crate::ControlState) -> T) -> Result<T>;
    /// Every entry, and the rows that did not read. (`Store::load_entries`).
    fn load_entries(&self) -> Result<(Vec<crate::Entry>, Vec<crate::Skipped>)>;
    /// The store's tag definitions. (`Store::store_tags`).
    fn store_tags(&self) -> Result<Vec<crate::StoreTag>>;
    /// Every pickup. (`Store::pickups`).
    fn pickups(&self) -> Result<Vec<crate::action::Pickup>>;
    /// Every mark. (`Store::all_marks`).
    fn all_marks(&self) -> Result<Vec<crate::annotation::MarkRecord>>;
    /// Write one entry. (`Store::write_entry`).
    fn write_entry(&self, entry: &crate::Entry, new: bool) -> Result<()>;
    /// Write the claims. (`Store::write_proposals`).
    fn write_proposals(&self, proposals: &[crate::Proposal]) -> Result<()>;
    /// Write the store tags. (`Store::write_tags`).
    fn write_tags(&self, tags: &[crate::StoreTag]) -> Result<()>;
    /// Resolve a session id or prefix. (`Store::resolve_session`).
    fn resolve_session(&self, input: &crate::SessionId) -> Result<crate::SessionId>;
    /// Resolve a session id or prefix, also among `extra_ids`. (`Store::resolve_session_or_typed_with`).
    fn resolve_session_or_typed_with(
        &self,
        input: &crate::SessionId,
        extra_ids: &[String],
    ) -> Result<crate::SessionId>;
    /// Resolve the session references in `related`, also among `extra_ids`. (`Store::resolve_relateds_with`).
    fn resolve_relateds_with(
        &self,
        related: &[crate::Related],
        extra_ids: &[String],
    ) -> Result<Vec<crate::Related>>;
    /// The entries written from one prompt of `session`. (`Store::entries_from_prompt`).
    fn entries_from_prompt(
        &self,
        session: &crate::SessionId,
        prompt_sha256: &str,
    ) -> Result<Vec<crate::Entry>>;
    /// Erase the stored messages `keys` name. (`Store::forget_messages`).
    fn forget_messages(&self, keys: &[(crate::SessionId, String)]) -> Result<usize>;
    /// Create `dir` owner-only, or check it is. (`Store::prepare_dir`).
    fn prepare_dir(&self, dir: &std::path::Path) -> Result<()>;
    /// The marks of `session`. (`Store::marks`).
    fn marks(&self, session: &crate::SessionId) -> Result<Vec<crate::annotation::MarkRecord>>;
    /// The memory files and their revisions. (`Store::memory_view`).
    fn memory_view(&self) -> Result<crate::MemoryView>;
    /// One memory file by name. (`Store::memory_file`).
    fn memory_file(&self, input: &str) -> Result<crate::MemoryShown>;
    /// Profile revision `n`. (`Store::profile_revision`).
    fn profile_revision(&self, n: i64) -> Result<crate::ProfileRevision>;
    /// The memory context a pass of `session` reads. (`Store::memory_context`).
    fn memory_context(
        &self,
        session: &crate::SessionId,
    ) -> Result<(
        Vec<crate::models::MemoryContext>,
        Vec<crate::models::PendingContext>,
    )>;
    /// Every session record. (`Store::load_sessions`).
    fn load_sessions(&self) -> Result<crate::LoadedSessions>;
    /// The definition of tag `word`, and where it comes from. (`Store::tag_definition`).
    fn tag_definition(&self, word: &str) -> Result<(crate::TagDef, &'static str)>;
    /// Record one audit event. (`Store::record_audit`).
    fn record_audit(&self, event: &crate::AuditEvent) -> Result<()>;
    /// Write a pass's memory changes outside a session write. (`Store::write_window_memory`).
    fn write_pass_memory(
        &self,
        pass: &crate::PassMemory,
        redactor: &crate::Redactor,
        record: &crate::SessionRecord,
    ) -> Result<crate::MemoryWritten>;
    /// A store write's connection (`Store::with_conn`).
    fn with_conn<T>(
        &self,
        f: impl FnOnce(&rusqlite::Connection, &std::path::Path) -> Result<T>,
    ) -> Result<T>;
}

impl StoreTestInternals for Store {
    fn lock_wait(&self) -> std::time::Duration {
        self.lock_wait
    }
    fn group(&self, id: &str) -> Result<crate::group::Group> {
        Store::group(self, id)
    }
    fn search_group_messages(
        &self,
        group: &str,
        tokens: &[String],
    ) -> Result<crate::message::MessageSearch> {
        Store::search_group_messages(self, group, tokens)
    }
    fn entry(&self, id: &crate::EntryId) -> Result<crate::Entry> {
        Store::entry(self, id)
    }
    fn proposals(&self) -> Result<Vec<crate::Proposal>> {
        Store::proposals(self)
    }
    fn audit_events(&self) -> Result<(Vec<crate::AuditEvent>, usize)> {
        Store::audit_events(self)
    }
    fn config(&self) -> Result<crate::Config> {
        Store::config(self)
    }
    fn list_entries(&self, filter: &crate::EntryFilter) -> Result<crate::EntryList> {
        Store::list_entries(self, filter)
    }
    fn list_sessions(&self, filter: &crate::SessionFilter) -> Result<crate::SessionList> {
        Store::list_sessions(self, filter)
    }
    fn session(&self, id: &crate::SessionId) -> Result<crate::SessionView> {
        Store::session(self, id)
    }
    fn control(&self) -> Result<crate::ControlState> {
        Store::control(self)
    }
    fn list_tags(&self) -> Result<crate::TagList> {
        Store::list_tags(self)
    }
    fn write_messages(
        &self,
        messages: &[crate::message::NewMessage],
        redactor: &crate::Redactor,
    ) -> Result<usize> {
        Store::write_messages(self, messages, redactor)
    }
    fn search_messages(&self, tokens: &[String]) -> Result<crate::message::MessageSearch> {
        Store::search_messages(self, tokens)
    }
    fn session_numbers(
        &self,
    ) -> Result<std::collections::BTreeMap<crate::SessionId, std::num::NonZeroU64>> {
        Store::session_numbers(self)
    }
    fn update_control<T>(&self, change: impl FnOnce(&mut crate::ControlState) -> T) -> Result<T> {
        Store::update_control(self, change)
    }
    fn load_entries(&self) -> Result<(Vec<crate::Entry>, Vec<crate::Skipped>)> {
        Store::load_entries(self)
    }
    fn store_tags(&self) -> Result<Vec<crate::StoreTag>> {
        Store::store_tags(self)
    }
    fn pickups(&self) -> Result<Vec<crate::action::Pickup>> {
        Store::pickups(self)
    }
    fn all_marks(&self) -> Result<Vec<crate::annotation::MarkRecord>> {
        Store::all_marks(self)
    }
    fn write_entry(&self, entry: &crate::Entry, new: bool) -> Result<()> {
        Store::write_entry(self, entry, new)
    }
    fn write_proposals(&self, proposals: &[crate::Proposal]) -> Result<()> {
        Store::write_proposals(self, proposals)
    }
    fn write_tags(&self, tags: &[crate::StoreTag]) -> Result<()> {
        Store::write_tags(self, tags)
    }
    fn resolve_session(&self, input: &crate::SessionId) -> Result<crate::SessionId> {
        Store::resolve_session(self, input)
    }
    fn resolve_session_or_typed_with(
        &self,
        input: &crate::SessionId,
        extra_ids: &[String],
    ) -> Result<crate::SessionId> {
        Store::resolve_session_or_typed_with(self, input, extra_ids)
    }
    fn resolve_relateds_with(
        &self,
        related: &[crate::Related],
        extra_ids: &[String],
    ) -> Result<Vec<crate::Related>> {
        Store::resolve_relateds_with(self, related, extra_ids)
    }
    fn entries_from_prompt(
        &self,
        session: &crate::SessionId,
        prompt_sha256: &str,
    ) -> Result<Vec<crate::Entry>> {
        Store::entries_from_prompt(self, session, prompt_sha256)
    }
    fn forget_messages(&self, keys: &[(crate::SessionId, String)]) -> Result<usize> {
        Store::forget_messages(self, keys)
    }
    fn prepare_dir(&self, dir: &std::path::Path) -> Result<()> {
        Store::prepare_dir(self, dir)
    }
    fn marks(&self, session: &crate::SessionId) -> Result<Vec<crate::annotation::MarkRecord>> {
        Store::marks(self, session)
    }
    fn memory_view(&self) -> Result<crate::MemoryView> {
        Store::memory_view(self)
    }
    fn memory_file(&self, input: &str) -> Result<crate::MemoryShown> {
        Store::memory_file(self, input)
    }
    fn profile_revision(&self, n: i64) -> Result<crate::ProfileRevision> {
        Store::profile_revision(self, n)
    }
    fn memory_context(
        &self,
        session: &crate::SessionId,
    ) -> Result<(
        Vec<crate::models::MemoryContext>,
        Vec<crate::models::PendingContext>,
    )> {
        Store::memory_context(self, session)
    }
    fn load_sessions(&self) -> Result<crate::LoadedSessions> {
        Store::load_sessions(self)
    }
    fn tag_definition(&self, word: &str) -> Result<(crate::TagDef, &'static str)> {
        Store::tag_definition(self, word)
    }
    fn record_audit(&self, event: &crate::AuditEvent) -> Result<()> {
        Store::record_audit(self, event)
    }
    fn write_pass_memory(
        &self,
        pass: &crate::PassMemory,
        redactor: &crate::Redactor,
        record: &crate::SessionRecord,
    ) -> Result<crate::MemoryWritten> {
        Store::write_window_memory(self, pass, redactor, record)
    }
    fn with_conn<T>(
        &self,
        f: impl FnOnce(&rusqlite::Connection, &std::path::Path) -> Result<T>,
    ) -> Result<T> {
        Store::with_conn(self, f)
    }
}

/// Store internals of `audit` that scribe-core's store tests reach.
pub mod audit {
    pub use crate::repository::audit::{AuditEvent, AuditStatus, IssueAudit, decode_formatted};
}

/// Store internals of `codec` that scribe-core's store tests reach.
pub mod codec {
    pub use crate::db::codec::{encode_entry, encode_session};
}

/// Store internals of `db` that scribe-core's store tests reach.
pub mod db {
    pub use crate::db::{
        DB_FILE, REMOVE_GROUP_CAPTURE_SCHEMA, db_error, execute, memory_view, meta,
        open_new_writer, open_writer, pending_read,
    };
    /// Store internals of `db::docs` that scribe-core's store tests reach.
    pub mod docs {
        pub use crate::db::docs::{put_audit, put_entry, serialized};
    }
}

/// Store internals of `memory` that scribe-core's store tests reach.
pub mod memory {
    pub use crate::repository::memory::CRASH_AFTER_COMMIT;
}

/// Store internals of `write` that scribe-core's store tests reach.
pub mod write {
    pub use crate::db::write::{AUDIT_SPOOL_DIR, BEFORE_SPOOL_DRAIN};
}

/// A temporary directory that is owner-only whatever the umask, as a store
/// directory must be (STD-05 R9). `tempfile` leaves the mode to the umask.
pub fn private_tempdir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
}

/// Whether any file under `root` (`scribe.db` and its WAL included) holds
/// `needle`: what a forget or a redaction must leave nowhere.
pub fn on_disk(root: &std::path::Path, needle: &str) -> bool {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for item in std::fs::read_dir(&dir).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if std::fs::read(&path)
                .unwrap()
                .windows(needle.len())
                .any(|w| w == needle.as_bytes())
            {
                return true;
            }
        }
    }
    false
}

/// The records [`seed`] writes.
#[derive(Default)]
pub struct Seed {
    /// The session records.
    pub sessions: Vec<crate::SessionRecord>,
    /// The switch state.
    pub control: crate::session::ControlState,
    /// The entries.
    pub entries: Vec<crate::Entry>,
    /// The claims.
    pub claims: Vec<crate::proposal::Proposal>,
    /// The store tags.
    pub tags: Vec<crate::StoreTag>,
    /// The marks.
    pub marks: Vec<crate::annotation::MarkRecord>,
    /// The pickups.
    pub pickups: Vec<crate::action::Pickup>,
    /// The audit lines, each with its format.
    pub audit: Vec<(u32, crate::AuditEvent)>,
}

/// A store at `root` (created owner-only if missing) holding `seed`'s
/// records as `scribe.db` rows, written through a store write as the
/// commands write them: every kind lives in the database, as in a store
/// any write since phase 8 left.
pub fn seed(root: &std::path::Path, seed: &Seed) -> super::Store {
    use super::db;
    if !root.exists() {
        std::fs::create_dir(root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let store = super::Store::open(root);
    store
        .with_conn(|conn, path| {
            let fail = |e| db::db_error("seed", path, e);
            for record in &seed.sessions {
                db::sessions::put_record(conn, path, record)?;
            }
            let before = crate::session::ControlState::default();
            db::sessions::put_control(conn, path, &before, &seed.control)?;
            for entry in &seed.entries {
                db::docs::put_entry(conn, path, entry, true, fail)?;
            }
            db::docs::put_claims(conn, path, &[], &seed.claims, fail)?;
            db::docs::put_tags(conn, path, &seed.tags, fail)?;
            db::docs::put_marks(conn, path, &seed.marks, fail)?;
            db::docs::put_pickups(conn, path, &seed.pickups, fail)?;
            for (format, event) in &seed.audit {
                db::docs::put_audit(conn, path, *format, event, None, fail)?;
            }
            Ok(())
        })
        .unwrap();
    store
}

/// A valid entry file for `preferences-1`, with `extra` frontmatter lines
/// inserted before the closing `---`.
pub fn entry_text(extra: &str) -> String {
    format!(
        "---
format: 1
kind: entry
id: preferences-1
tag: preferences
status: active
project: scribe
topics: [ci]
related: [\"repo:scribe\"]
provider: claude-code
host: host
created: 2026-11-01T10:00:00Z
updated: 2026-11-01T10:00:00Z
said: \"#scribe preferences: run the gate\"
content_from: {{kind: instruction}}
cite: {{session: s-1, message: null, at: 2026-11-01T10:00:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /work}}
superseded_by: null
review_after: null
{extra}---
run the gate before every push
",
        sha = "0".repeat(64)
    )
}

/// A valid session record for `s-1`, started in November 2026.
pub fn session_text(front_extra: &str, body: &str) -> String {
    format!(
        "---
format: 1
kind: session
id: s-1
status: open
project: scribe
topics: []
related: []
terms: [{{term: level 4, said: \"graduate to level 4\", message: m-1}}]
provider: claude-code
host: host
created: 2026-11-01T10:00:00Z
updated: 2026-11-01T10:00:00Z
title: a session
cwd: /work
started: 2026-11-01T09:00:00Z
on_since: 2026-11-01T09:00:00Z
scribed_through: null
passes: 1
next_open: 0
{front_extra}---
{body}"
    )
}

/// The body of [`session_text`]'s record: every header section filled.
pub const SESSION_BODY: &str =
    "## Summary\nsum\n\n## Goals\ngoal\n\n## Outcome\nout\n\n## Next\n- one\n- two\n";
