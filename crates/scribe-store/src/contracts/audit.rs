//! Audit log record shapes: one JSON line per mutating command.
//!
//! An event holds stable identifiers only: the command path, the record it
//! acted on, the outcome and a stable error code. Never argument values or
//! error text, which can carry record contents or paths (STD-05 R13).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// The audit log's file under the data directory, before it moved into
/// `scribe.db`.
pub const AUDIT_FILE: &str = "audit.jsonl";

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
