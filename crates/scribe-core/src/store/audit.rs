//! The audit log's record shape: one JSON line per mutating command.
//!
//! `<root>/audit.jsonl` answers "what changed this store, when, and did it
//! work". The CLI's audit middleware builds each [`AuditEvent`] and
//! [`crate::Store::record_audit`] appends it. Read-only commands are never
//! audited: they must not write (STD-01 R31).
//!
//! An event holds stable identifiers only: the command path, the record it
//! acted on, the outcome and a stable error code. Never argument values or
//! error text, which can carry record contents or paths (STD-05 R13). Each line
//! carries its own `format` so a reader can refuse a newer one (STD-03 R10);
//! a crash can leave a torn last line, which a reader skips.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// The audit log's name under the data directory.
pub const AUDIT_FILE: &str = "audit.jsonl";

/// The newest audit line format. Bump it, and keep reading the old one, when
/// the line's shape changes (STD-02 R16). Format 2 adds `author`; a format 1
/// line reads with none. Format 3 adds `instruction_session` and
/// `instruction_message`; an older line reads with neither. Format 4 adds
/// `issue` (2_design.md §11.4); an older line reads with none.
pub(super) const AUDIT_FORMAT: u32 = 4;

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

/// The ids an issue write or forget is counted by (2_design.md §11.4): one
/// per audit line, never free text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueAudit {
    /// `add`, `occur`, `update`, `move` or `forget`.
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
}

/// A persisted line: the event plus its format.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct AuditLine {
    pub(super) format: u32,
    #[serde(flatten)]
    pub(super) event: AuditEvent,
}

/// The event as one newline-terminated JSON line at the current format.
pub(super) fn encode_line(event: &AuditEvent) -> serde_json::Result<Vec<u8>> {
    let mut line = serde_json::to_vec(&AuditLine {
        format: AUDIT_FORMAT,
        event: event.clone(),
    })?;
    line.push(b'\n');
    Ok(line)
}
