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

use crate::issue_trigger::FIRE_VERB;
use crate::record::DateRange;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

/// The audit log's name under the data directory.
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

/// The ids an issue write, forget or trigger fire is counted by
/// (2_design.md §11.4): one per audit line, never prompt or issue text.
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

/// The events of an audit log, oldest first. A line that does not parse
/// (a torn last line, a hand edit) is skipped and counted; a line of a newer
/// format refuses the whole read, so no count is made from a log this build
/// cannot read.
pub(super) fn decode_lines(
    path: &std::path::Path,
    bytes: &[u8],
) -> crate::Result<(Vec<AuditEvent>, usize)> {
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
        events.push(line.event);
    }
    Ok((events, skipped))
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

/// What `scribe issue stats` counts. Each ratio is a proxy, named as one:
/// see [`IssueStats::precision`] and [`IssueStats::trigger_yield`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssueStats {
    /// The first day counted, if one was given.
    pub since: Option<Date>,
    /// Trigger fires.
    pub fires: u64,
    /// Fires followed by an issue write whose request is the same turn.
    pub fires_with_write: u64,
    /// Issue writes (`add` and `occur`), each one occurrence.
    pub writes: u64,
    /// Writes whose request turn no fire matched.
    pub writes_without_fire: u64,
    /// Writes whose occurrence, or whole issue, was later forgotten.
    pub writes_forgotten: u64,
    /// Writes whose occurrence was later moved to another issue.
    pub writes_moved: u64,
    /// Writes later forgotten or moved, each counted once.
    pub writes_withdrawn: u64,
    /// Audit lines that did not parse and were left out.
    pub skipped_lines: usize,
}

impl IssueStats {
    /// Precision proxy: writes neither forgotten nor moved ÷ writes;
    /// `None` with no write.
    #[must_use]
    pub fn precision(&self) -> Option<(u64, u64)> {
        (self.writes > 0).then(|| (self.writes - self.writes_withdrawn, self.writes))
    }

    /// Trigger-yield proxy: fires that led to a write ÷ fires; `None` with
    /// no fire.
    #[must_use]
    pub fn trigger_yield(&self) -> Option<(u64, u64)> {
        (self.fires > 0).then_some((self.fires_with_write, self.fires))
    }
}

/// A write as the log records it: the occurrence written and its request.
struct Write<'a> {
    at: OffsetDateTime,
    id: &'a str,
    occurrence: u32,
    audit: &'a IssueAudit,
}

/// The successful issue note of `event` with verb `verb`.
fn issue_note<'a>(event: &'a AuditEvent, verbs: &[&str]) -> Option<&'a IssueAudit> {
    let issue = event.issue.as_ref()?;
    (event.status == AuditStatus::Success && verbs.contains(&issue.verb.as_str())).then_some(issue)
}

/// Whether a fire and a write name the same request turn: one session and
/// a shared sha256.
fn same_request(fire: &IssueAudit, write: &IssueAudit) -> bool {
    fire.session.is_some()
        && fire.session == write.session
        && fire
            .request_sha256
            .iter()
            .any(|sha| write.request_sha256.contains(sha))
}

/// Count `events` (the audit log, oldest first) for `issue stats`
/// (2_design.md §11.4). Fires and writes count from
/// `since` on; forgets and moves count whenever they came after the write.
#[must_use]
pub fn issue_stats(events: &[AuditEvent], since: Option<Date>, skipped_lines: usize) -> IssueStats {
    let range = DateRange { since, until: None };
    let fires: Vec<(OffsetDateTime, &IssueAudit)> = events
        .iter()
        .filter_map(|e| issue_note(e, &[FIRE_VERB]).map(|n| (e.at, n)))
        .collect();
    let writes: Vec<Write<'_>> = events
        .iter()
        .filter_map(|e| {
            let audit = issue_note(e, &["add", "occur"])?;
            Some(Write {
                at: e.at,
                id: audit.id.as_deref()?,
                occurrence: audit.occurrence?,
                audit,
            })
        })
        .collect();
    let withdrawn = |write: &Write<'_>, verb: &str| {
        events.iter().any(|e| {
            e.at >= write.at
                && issue_note(e, &[verb]).is_some_and(|n| {
                    n.id.as_deref() == Some(write.id)
                        && (n.occurrence.is_none() && verb == "forget"
                            || n.occurrence == Some(write.occurrence))
                })
        })
    };
    let mut stats = IssueStats {
        since,
        skipped_lines,
        ..IssueStats::default()
    };
    for (at, fire) in fires.iter().filter(|(at, _)| range.contains(*at)) {
        stats.fires += 1;
        if writes
            .iter()
            .any(|w| w.at >= *at && same_request(fire, w.audit))
        {
            stats.fires_with_write += 1;
        }
    }
    for write in writes.iter().filter(|w| range.contains(w.at)) {
        stats.writes += 1;
        if !fires
            .iter()
            .any(|(at, fire)| *at <= write.at && same_request(fire, write.audit))
        {
            stats.writes_without_fire += 1;
        }
        let forgotten = withdrawn(write, "forget");
        let moved = withdrawn(write, "move");
        stats.writes_forgotten += u64::from(forgotten);
        stats.writes_moved += u64::from(moved);
        stats.writes_withdrawn += u64::from(forgotten || moved);
    }
    stats
}
