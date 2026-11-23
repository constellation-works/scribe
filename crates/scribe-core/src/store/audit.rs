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

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use time::OffsetDateTime;

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
pub(super) const AUDIT_FORMAT: u32 = 5;

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
pub(super) struct AuditLine {
    pub(super) format: u32,
    #[serde(flatten)]
    pub(super) event: AuditEvent,
}

/// One parsed line of `audit.jsonl` with its byte-offset/content key
/// (`audit.jsonl:<offset>:<sha256>`) for idempotent import into `scribe.db`.
pub(super) struct ImportedLine {
    pub(super) format: u32,
    pub(super) event: AuditEvent,
    pub(super) key: String,
}

/// The events of an audit log, oldest first, each with its own format and
/// a deterministic key (`audit.jsonl:<offset>:<sha256>`) derived from its
/// byte offset and raw line bytes. A line that does not parse (a torn last
/// line, a hand edit) is skipped and counted; a line of a newer format
/// refuses the whole read, so no count is made from a log this build cannot
/// read.
pub(super) fn decode_for_import(
    path: &std::path::Path,
    bytes: &[u8],
) -> crate::Result<(Vec<ImportedLine>, usize)> {
    let mut events = Vec::new();
    let mut skipped = 0usize;
    let mut offset = 0usize;
    for raw in bytes.split(|b| *b == b'\n') {
        let line_offset = offset;
        offset = offset.saturating_add(raw.len()).saturating_add(1);
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
        events.push(ImportedLine {
            format: line.format,
            event: line.event,
            key: import_key(line_offset, raw),
        });
    }
    Ok((events, skipped))
}

fn import_key(offset: usize, raw: &[u8]) -> String {
    let digest = Sha256::digest(raw);
    let mut key = format!("{AUDIT_FILE}:{offset}:");
    for byte in digest {
        let _ = write!(key, "{byte:02x}");
    }
    key
}

/// The events of an audit log, oldest first, each with its own format,
/// which a move into `scribe.db` keeps. A line that does not parse (a torn
/// last line, a hand edit) is skipped and counted; a line of a newer format
/// refuses the whole read, so no count is made from a log this build cannot
/// read.
pub(super) fn decode_formatted(
    path: &std::path::Path,
    bytes: &[u8],
) -> crate::Result<(Vec<(u32, AuditEvent)>, usize)> {
    let (lines, skipped) = decode_for_import(path, bytes)?;
    Ok((
        lines.into_iter().map(|l| (l.format, l.event)).collect(),
        skipped,
    ))
}

/// The event as one newline-terminated JSON line with its format.
pub(super) fn encode_line(format: u32, event: &AuditEvent) -> serde_json::Result<Vec<u8>> {
    let mut line = serde_json::to_vec(&AuditLine {
        format,
        event: event.clone(),
    })?;
    line.push(b'\n');
    Ok(line)
}
