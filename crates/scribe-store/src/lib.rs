//! Persistence: the scribe store (`scribe.db` and the memory files) and
//! everything that touches disk.
//!
//! The business logic is in `scribe-domain`, with no I/O (`entry`, `tag`,
//! `session`, `proposal`, `config`, `verbatim`); this crate is the only one
//! that reads or writes the data root, so a second backend replaces this
//! crate and nothing else. `scribe-core` re-exports it as its `store`
//! module. It is split by responsibility:
//!
//! - this file, the [`Store`] facade: open the root, its layout and the
//!   listing's skipped records;
//! - `db/`: `scribe.db`, the SQLite database of every record kind
//!   (2_design.md §15.7), its row access, and the storage mechanism under
//!   it; the only user of `rusqlite`:
//!   - `db/codec`: the current format's documents, as the export, the memory
//!     files and rebuild checkpoints hold them, and the row checks;
//!   - `db/import`: the file store's archives in `scribe.db`, imported once
//!     and erased from by `forget` (phase 8);
//!   - `db/write`: a store write as one `scribe.db` transaction, the archive
//!     import, and audit lines that never wait;
//!   - `db/locks`: the session and Codex-ingestion locks and rebuild
//!     checkpoints;
//!   - `db/fsio`: durable writes, locking and private-state checks,
//!     mechanism only;
//!   - `db/documents`: the documents as a whole: whether they moved in, the
//!     format stamps, the first write's import and their part of `scribe
//!     export`;
//! - `repository/`: one repository per record (entries, sessions,
//!   proposals, tags, groups, memory, messages, marks, pickups, audit), each
//!   with the reads and writes under it, and `ids`, the id resolution they
//!   share.
//! - `contracts/`: the record shapes, with no database or repository
//!   access (empty until step 5 moves them in).
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
//! <root>/locks/                             advisory locks (Codex catalog, rebuild)
//! ```
//!
//! The file store's `pre-sqlite-<date>/` and `pre-issue-<date>/` archives
//! are imported into `scribe.db` at the first write and then removed; a
//! file store whose records never moved in is refused, since this build has
//! no file decoder (2_design.md §15.7, phase 8).
//!
//! Reads take no lock and write nothing (STD-01 R31): SQLite's WAL gives a
//! reader a committed snapshot, and the CLI works on a read-only root. Every
//! load refuses state that is a link, another user's, or open to any group or
//! other mode bit (STD-05 R7–R9), and a newer format (STD-03 R10). A record
//! that does not read is refused naming it; a listing skips it and reports it
//! in [`Skipped`], so one bad record does not hide the rest (4_decisions.md,
//! "One malformed record makes its file corrupt").

#![deny(clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

mod contracts;
// The group shapes at the path the store's code names them by.
use contracts::group;
mod db;
mod error;
mod redact;
pub use db::documents::Imported;
pub use db::fsio;
pub use db::locks::SessionLock;
pub use db::write::ImportFacts;
pub use db::{ClaudeCapturePosition, ClaudeGroupCapture, DB_FILE};
pub mod repository;
pub use contracts::action::Pickup;
pub use contracts::group::{Group, LastGroup, NewGroup, Seat, SeatHold};
pub use contracts::mark::{AGENT_HANDOFF, MAX_ATTEMPTS, MODEL_FAILED, MarkRecord, MarkState};
pub use error::{forgotten_refusal, refusal};
pub use repository::audit::{AUDIT_FILE, AuditEvent, AuditStatus, IssueAudit};
pub use repository::entries::EntryList;
pub use repository::ids::{number as group_number, resolve_prefix};
pub use repository::memory::{
    MEMORY_DIR, MILESTONES_DIR, MemoryShown, MemoryStep, MemoryView, MemoryWritten, NOW_DIR,
    PROFILE_FILE, ProfileRevision,
};
pub use repository::memory::{bare, not_found};
pub use repository::sessions::{
    ExportCounts, ExportFormat, LOCKS_DIR, LoadedSessions, SessionErasure, SessionForget,
    SessionRow,
};
pub use repository::{Snapshot, Tx};
#[cfg(feature = "test-support")]
mod seed;

pub use redact::{MIN_SECRET_CHARS, REDACTED_FIELDS, Redactor, unresolved_redact_env};

// The store's own tests reach the audit record shape at its old path.
#[cfg(test)]
use repository::audit;
// The domain at the paths this code had as scribe-core's `store` module.
#[cfg(test)]
use crate as store;
use crate::config::Config;
use crate::entry::{Entry, EntryFilter, EntryId};
use crate::error::{Error, Result};
use crate::proposal::Proposal;
use crate::session::{ControlState, SessionId, SessionRecord};
use scribe_domain::tag::{self};
use scribe_domain::{
    Author, AuthorKind, ClosedReason, ContentFrom, EntryStatus, Instruction, Label, LinkType,
    Project, ProposalId, ProposalKind, Related, RelatedKind, SessionError, SessionStatus, Speaker,
    StoreTag, TagDef, TagTable, WriteRefusal,
};
use scribe_domain::{
    capture, config, entry, handoff, instruction, message, models, proposal, record, session,
};
// The domain names the redaction tests (`tests/redact.rs`) reach at the
// crate root, as they did in the domain.
#[cfg(test)]
use scribe_domain::{
    Admitted, AnswerIn, AuthorName, Decision, EntryCandidate, ItemAnswer, ItemClosure, OpenItem,
    Outcome, OutcomeState, Window, WindowTurn,
};
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

/// The legacy issue records' directory: a file store kind removed in
/// phase 2, refused like the rest of a file store.
const ISSUES_DIR: &str = "issues";

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
}

/// Names that can be records: not hidden. A writer stages its temporary file
/// as a dot-file beside the target, and an editor leaves swap files the
/// same way; neither is a record.
fn visible(names: Vec<String>) -> impl Iterator<Item = String> {
    names.into_iter().filter(|n| !n.starts_with('.'))
}

#[cfg(any(test, feature = "test-support"))]
pub mod testing;
#[cfg(test)]
mod tests;
