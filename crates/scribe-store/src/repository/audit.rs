//! The audit log's record shape: one JSON line per mutating command.
//!
//! The `audit` table of `scribe.db` (`<root>/audit.jsonl` before
//! 2_design.md §15.9 phase 7) answers "what changed this store, when, and
//! did it work". The CLI's audit middleware builds each [`AuditEvent`] and
//! [`crate::Store::record_audit`] records it. Read-only commands are never
//! audited: they must not write (STD-01 R31).
//!
//! An event holds stable identifiers only: the command path, the record it
//! acted on, the outcome and a stable error code. Never argument values or
//! error text, which can carry record contents or paths (STD-05 R13). Each line
//! carries its own `format` so a reader can refuse a newer one (STD-03 R10);
//! a crash can leave a torn last line, which a reader skips.

use super::Audit;
use crate::Store;
use crate::db::{self, docs};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use time::OffsetDateTime;

impl Audit<'_> {
    /// Every audit event and how many lines did not read.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn events(&self) -> Result<(Vec<AuditEvent>, usize)> {
        self.store.audit_events()
    }

    /// Record one audit line. Inside a write's transaction it commits with
    /// the write. Otherwise it is inserted without waiting, and spooled to
    /// a file of its own when another writer holds the database or the
    /// store is still files, so no line waits on a lock or is lost to one.
    ///
    /// A store too new for this build takes no line.
    ///
    /// # Errors
    ///
    /// An unsafe data directory, or a spool file that cannot be written.
    pub fn record(&self, event: &AuditEvent) -> Result<()> {
        if self.store.open_conn().is_some() {
            return self.store.with_conn(|conn, path| {
                docs::put_audit(conn, path, AUDIT_FORMAT, event, None, |e| {
                    db::db_error("audit to", path, e)
                })
                .map(drop)
            });
        }
        self.store.prepare_dir(&self.store.root)?;
        let moved = match self.store.documents_moved() {
            Ok(moved) => moved,
            // A store a newer build migrated takes no write from this one,
            // its log included; the command already reported the refusal.
            Err(Error::NewerFormat { .. }) => return Ok(()),
            Err(other) => return Err(other),
        };
        if moved {
            match self.store.audit_without_waiting(event) {
                Ok(()) => return Ok(()),
                Err(Error::Locked { .. }) => {}
                Err(other) => return Err(other),
            }
        }
        self.store.spool_audit(event)
    }

    /// The audit log, `<root>/audit.jsonl`.
    #[must_use]
    pub fn path(&self) -> std::path::PathBuf {
        self.store.audit_path()
    }
}

impl Store {
    /// The audit log, `<root>/audit.jsonl`.
    #[must_use]
    pub(crate) fn audit_path(&self) -> PathBuf {
        self.root.join(AUDIT_FILE)
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
    pub(crate) fn audit_lines(&self) -> Result<(Vec<(u32, AuditEvent)>, usize)> {
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
}

/// The audit log's file under the data directory, before it moved into
/// `scribe.db`.
pub const AUDIT_FILE: &str = "audit.jsonl";

/// The newest audit line format. Bump it, and keep reading the old one, when
/// the line's shape changes (STD-02 R16). Format 2 adds `author`; a format 1
/// line reads with none. Format 3 adds `instruction_session` and
/// `instruction_message`; an older line reads with neither. Format 4 adds
/// `issue` (2_design.md §11.4); an older line reads with none. Format 5 adds
/// `issue.trigger` and `issue.request_sha256`, for the prompt hook's trigger
/// fires and the writes they are matched to; an older line reads with
/// neither.
pub(crate) const AUDIT_FORMAT: u32 = 5;

/// How an audited command ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditStatus {
    /// The command completed.
    Success,
    /// The command returned an error, or never reported a result (a panic or
    /// an early return), which reads as failure, never as success (STD-02 R29).
    Failure,
}

/// One audited command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// When the command started.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The command path, such as `hook user-prompt-submit`.
    pub command: String,
    /// How it ended.
    pub status: AuditStatus,
    /// The id of the record it acted on, when it names one.
    #[serde(default)]
    pub target: Option<String>,
    /// The stable error code on failure (STD-01 R19); never the message.
    #[serde(default)]
    pub error_code: Option<String>,
    /// The agent's `--author` name, for a command that takes one, on success
    /// and refusal alike. A self-reported identity, kept apart from the
    /// command's arguments (STD-05 R2); `null` when none was given.
    #[serde(default)]
    pub author: Option<String>,
    /// For a write run on Daniel's instruction (`--session`), the session
    /// it was looked for in (2_design.md §12.4); `null` otherwise. Never the
    /// instruction's words.
    #[serde(default)]
    pub instruction_session: Option<String>,
    /// The uuid of the turn that backs that instruction, once the check
    /// found it; `null` when it did not get that far, or without one.
    #[serde(default)]
    pub instruction_message: Option<String>,
    /// For an issue write or a forget of an issue or an occurrence, what it
    /// counted against: the verb, the issue, the occurrence, the session and
    /// the request's message; `null` otherwise. Ids only, so recall and
    /// precision can be counted from the log without any of the issue's
    /// words.
    #[serde(default)]
    pub issue: Option<IssueAudit>,
    /// Wall time from start to result, in milliseconds.
    pub duration_ms: u64,
}

/// The ids an issue write, forget or trigger fire was counted by
/// (2_design.md §11.4): one per audit line, never prompt or issue text.
/// Issue records were removed in 2_design.md §15.9 phase 2; lines written
/// before then keep this field as history, and nothing reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueAudit {
    /// `add`, `occur`, `update`, `move`, `forget`, or `fire` for the prompt
    /// hook's `issue` trigger firing on one of Daniel's typed turns.
    pub verb: String,
    /// The issue it acted on, once known: for a move, the source.
    #[serde(default)]
    pub id: Option<String>,
    /// The occurrence it wrote, moved or forgot; `null` for a whole issue
    /// or an update.
    #[serde(default)]
    pub occurrence: Option<u32>,
    /// The session of the request it ran on: the `--related session:<id>`
    /// of a write, or the session the `#scribe forget` line was typed in.
    #[serde(default)]
    pub session: Option<String>,
    /// The uuid of Daniel's typed turn holding the request, once the quote
    /// check found it; `null` for a forget, typed in the turn the hook is
    /// answering.
    #[serde(default)]
    pub request_message: Option<String>,
    /// For a move, where the occurrence went (`issue-<n>#<m>`).
    #[serde(default)]
    pub moved_to: Option<String>,
    /// For a fire, the `issue` trigger phrase that fired, as configured
    /// (format 5); `null` otherwise. Config text, never the prompt's words.
    #[serde(default)]
    pub trigger: Option<String>,
    /// The sha256 of the request turn, by which `issue stats` matches a
    /// fire to the write that answered it (format 5). A fire records its
    /// prompt's; an `add` or `occur` records those of the turn its request
    /// was found in (the whole text and each text block, as the `Stop`
    /// hook matches cites). At `UserPromptSubmit` the turn has no
    /// transcript uuid yet, so the hash is the shared key. Empty otherwise.
    #[serde(default)]
    pub request_sha256: Vec<String>,
}

/// A persisted line: the event plus its format.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AuditLine {
    pub(crate) format: u32,
    #[serde(flatten)]
    pub(crate) event: AuditEvent,
}

/// The events of an audit log or a spool file, oldest first, each with its
/// own format, which the `audit` table keeps. A line that does not parse (a
/// torn last line, a hand edit) is skipped and counted; a line of a newer
/// format refuses the whole read, so no count is made from a log this build
/// cannot read.
pub fn decode_formatted(
    path: &std::path::Path,
    bytes: &[u8],
) -> crate::Result<(Vec<(u32, AuditEvent)>, usize)> {
    let mut events = Vec::new();
    let mut skipped = 0usize;
    for raw in bytes.split(|b| *b == b'\n') {
        if raw.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(line) = serde_json::from_slice::<AuditLine>(raw) else {
            skipped += 1;
            continue;
        };
        if line.format > AUDIT_FORMAT {
            return Err(crate::Error::NewerFormat {
                path: path.to_path_buf(),
                found: line.format,
                supported: AUDIT_FORMAT,
            });
        }
        events.push((line.format, line.event));
    }
    Ok((events, skipped))
}

/// The event as one newline-terminated JSON line with its format.
pub(crate) fn encode_line(format: u32, event: &AuditEvent) -> serde_json::Result<Vec<u8>> {
    let mut line = serde_json::to_vec(&AuditLine {
        format,
        event: event.clone(),
    })?;
    line.push(b'\n');
    Ok(line)
}
