//! Persistence: the scribe store (`scribe.db` and the memory files) and
//! everything that touches disk.
//!
//! The record shapes are this crate's `contracts/` (`entry`, `tag`,
//! `session`, `proposal`, `record`, …), with no I/O, and the rules over
//! them are scribe-core's apps; this crate is the only one
//! that reads or writes the data root, so a second backend replaces this
//! crate and nothing else. `scribe-core` re-exports it as its `store`
//! module. It is split by responsibility:
//!
//! - this file, the [`Store`] facade: open the root, its layout and the
//!   listing's skipped records;
//! - `db/`: `scribe.db`, the SQLite database of every record kind
//!   (2_design.md §15.7), opened, configured and migrated through `cw-sqlite`,
//!   its row access, and the storage mechanism under it; the only user of
//!   `cw-sqlite` and its re-exported `rusqlite`:
//!   - `db/codec`: the current format's documents, as the export, the memory
//!     files and rebuild checkpoints hold them, and the row checks;
//!   - `db/import`: the file store's archives in `scribe.db`, imported once
//!     and erased from by `forget` (phase 8);
//!   - `db/write`: a store write as one `scribe.db` transaction, the archive
//!     import, and audit lines that never wait;
//!   - `db/locks`: the session and Codex-ingestion locks and rebuild
//!     checkpoints;
//!   - `db/files`: the store's use of `cw-fs` (caps, the root link),
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
mod db;
pub mod digest;
pub mod error;
#[cfg(any(test, feature = "test-support"))]
pub mod fixtures;
mod redact;
pub mod repository;
#[cfg(feature = "test-support")]
mod seed;
/// The session shapes (the store's contracts) and the log check the store
/// applies to every record it reads or writes (`db/log.rs`), at one path:
/// `scribe_store::session::SessionRecord`, `scribe_store::session::check_log`.
pub mod session {
    pub use crate::contracts::session::*;
    pub use crate::db::log::{LogError, check_log};
}
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use contracts::action::Pickup;
pub use contracts::audit::{AUDIT_FILE, AuditEvent, AuditStatus, IssueAudit};
pub use contracts::group::number as group_number;
pub use contracts::group::{ClaudeSeen, Group, LastGroup, NewGroup, Seat, SeatHold};
pub use contracts::mark::{AGENT_HANDOFF, MAX_ATTEMPTS, MODEL_FAILED, MarkRecord, MarkState};
pub use contracts::memory::{
    MemoryContext, Milestone, MilestoneFront, NowFront, NowItem, NowKind, Pending, PendingContext,
    ProfileView, Raised, Refusal, SLUG_CHARS, Stem, TEXT_CHARS, TITLE_CHARS,
};
pub use contracts::message::{MessageMatch, MessageSearch, NewMessage};
pub use contracts::proposal::{
    Admitted, CONTEXT_CHARS, CONTEXT_SENTENCES, ClosedReason, EntryCandidate,
    INSTRUCTED_SAID_PREFIX, Proposal, ProposalError, ProposalId, ProposalKind, ProposalState,
    Quote, SUBJECT_CHARS,
};
use contracts::tag;
pub use contracts::tag::{
    ActionTag, CAPTURE_TEXT_CHARS, CaptureFrom, CaptureRoute, CaptureRules, FINDING_TAG, Rule,
    RuleOrigin, StoreTag, TagDef, TagError, TagTable,
};
pub use db::documents::{ExportCounts, ExportFormat};
pub use db::locks::SessionLock;
pub use db::write::{ImportFacts, Imported};
pub use db::{ClaudeCapturePosition, ClaudeGroupCapture, DB_FILE};
pub use error::{Error, Result, WriteRefusal, forgotten_refusal, refusal};
pub use provider::Provider;
pub use redact::{MIN_SECRET_CHARS, REDACTED_FIELDS, Redactor};
pub use repository::entries::EntryList;
pub use repository::memory::{
    MILESTONES_DIR, MemoryShown, MemoryStep, MemoryView, MemoryWritten, NOW_DIR, PROFILE_FILE,
    ProfileRevision,
};
pub use repository::memory::{bare, not_found};
pub use repository::sessions::{
    LOCKS_DIR, LoadedSessions, SessionErasure, SessionForget, SessionRow,
};
pub use repository::{Snapshot, Tx};

// The entry and handoff shapes, as modules: callers name them by path
// (`scribe_store::entry::Entry`, `scribe_store::handoff::MAX_CHARS`).
pub use contracts::{entry, handoff, provider, record};

// The group shapes at the path the store's code names them by.
use contracts::group;

// The message shapes at the path the store's code names them by.
use contracts::message;

// The proposal shapes at the path the store's code names them by.
use contracts::proposal;

// The store's own tests reach the audit record shape at its old path.
#[cfg(test)]
use repository::audit;
// The domain at the paths this code had as scribe-core's `store` module.
#[cfg(test)]
use crate as store;
use crate::db::files;
use crate::entry::{ContentFrom, EntryStatus, Instruction, Speaker};
use crate::entry::{Entry, EntryFilter, EntryId};
use crate::record::{Author, AuthorKind, Label, Project, Related, RelatedKind};
use crate::session::{
    ControlState, LinkType, SessionError, SessionId, SessionRecord, SessionStatus,
};
// The domain names the redaction tests (`tests/redact.rs`) and their
// shared fixtures (`testing/redact.rs`) reach at the crate root, as they did
// in the domain.
#[cfg(any(test, feature = "test-support"))]
use crate::entry::{AnswerIn, Decision, ItemAnswer, ItemClosure, OpenItem, Outcome, OutcomeState};
#[cfg(test)]
use crate::entry::{Window, WindowTurn};
// The session tests (`contracts/tests/session.rs`) name it at the root, as
// they did in the domain.
#[cfg(any(test, feature = "test-support"))]
use crate::record::AuthorName;
#[cfg(test)]
use crate::session::ShortIds;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The configuration file's name under the data directory.
pub const CONFIG_FILE: &str = "config.toml";
/// The entries directory's name under the data directory.
pub const ENTRIES_DIR: &str = "entries";
/// The session records directory's name under the data directory.
pub const SESSIONS_DIR: &str = "sessions";
/// The memory directory's name under the data directory.
pub const MEMORY_DIR: &str = "memory";
/// The control document's name under the data directory.
pub const CONTROL_FILE: &str = "control.json";
/// The proposals document's name under the data directory.
pub const PROPOSALS_FILE: &str = "proposals.json";
/// The store tags' stamp file.
pub(crate) const TAGS_FILE: &str = "tags.json";
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
/// `cw-sqlite`'s lock wait on `scribe.db`, and the per-session locks' wait.
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(10);

/// The shortest prefix accepted for id resolution, in characters.
pub(crate) const MIN_PREFIX: usize = crate::session::MIN_ID_PREFIX;

/// The most candidates an `id_ambiguous` refusal names.
pub(crate) const MAX_CANDIDATES: usize = 10;

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
        if !files::inspect_root(dir)? {
            cw_fs::create_private_dir(dir)?;
        }
        Ok(())
    }

    fn prepare_subdir(&self, dir: &Path) -> Result<()> {
        if !cw_fs::inspect_dir(dir)? {
            cw_fs::create_private_dir(dir)?;
        }
        Ok(())
    }

    fn root_exists(&self) -> Result<bool> {
        files::inspect_root(&self.root)
    }
}

/// Names that can be records: not hidden. A writer stages its temporary file
/// as a dot-file beside the target, and an editor leaves swap files the
/// same way; neither is a record.
fn visible(names: Vec<String>) -> impl Iterator<Item = String> {
    names.into_iter().filter(|n| !n.starts_with('.'))
}

/// The index in `ids` of `input`: the id equal to it, else the one id it is
/// a prefix of (at least [`MIN_PREFIX`] characters). `None` means nothing
/// matched; the caller builds its noun's not-found error, which names the id
/// and the place searched (STD-02 R26).
///
/// # Errors
///
/// `usage_error` when `input` is not an id and is shorter than
/// [`MIN_PREFIX`] characters; `id_ambiguous` listing the candidates when
/// several ids match.
pub(crate) fn resolve_id_index(input: &str, ids: &[String]) -> Result<Option<usize>> {
    if let Some(i) = ids.iter().position(|id| id == input) {
        return Ok(Some(i));
    }
    if input.chars().count() < MIN_PREFIX {
        return Err(refusal(
            WriteRefusal::InvalidId,
            format!(
                "'{input}' is not an id and too short to be a prefix; give an id or a unique prefix of at least four characters"
            ),
        ));
    }
    let matches: Vec<usize> = ids
        .iter()
        .enumerate()
        .filter(|(_, id)| id.starts_with(input))
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [i] => Ok(Some(*i)),
        [] => Ok(None),
        _ => Err(ambiguous_id(
            input,
            matches.iter().map(|i| ids[*i].as_str()),
        )),
    }
}

/// The id in `ids` that `input` names: exact, else a unique prefix of at
/// least [`MIN_PREFIX`] characters. `None` when none does, so the caller can
/// report its own noun's not-found with the place it searched.
///
/// # Errors
///
/// `usage_error` for an input shorter than [`MIN_PREFIX`] that is not an id;
/// `id_ambiguous` when several ids match.
pub fn resolve_prefix(input: &str, ids: &[String]) -> Result<Option<String>> {
    Ok(resolve_id_index(input, ids)?.map(|i| ids[i].clone()))
}

fn ambiguous_id<'a>(input: &str, candidates: impl Iterator<Item = &'a str>) -> Error {
    let mut candidates: Vec<&str> = candidates.collect();
    candidates.sort_unstable();
    let more = candidates.len().saturating_sub(MAX_CANDIDATES);
    candidates.truncate(MAX_CANDIDATES);
    let mut message = format!(
        "id prefix '{input}' is ambiguous; candidates: {}",
        candidates.join(", ")
    );
    if more > 0 {
        message.push_str(&format!(" and {more} more"));
    }
    message.push_str("; give more characters of the id");
    refusal(WriteRefusal::IdAmbiguous, message)
}

#[cfg(test)]
mod tests;
