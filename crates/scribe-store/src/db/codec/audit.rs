//! Audit log line codec: one JSON line per mutating command, carrying its own
//! format version so a reader can refuse a newer one (STD-03 R10). Bytes in,
//! events out: no I/O.

use crate::contracts::audit::AuditEvent;
use serde::{Deserialize, Serialize};

/// The newest audit line format. Bump it, and keep reading the old one, when
/// the line's shape changes (STD-02 R16). Format 2 adds `author`; a format 1
/// line reads with none. Format 3 adds `instruction_session` and
/// `instruction_message`; an older line reads with neither. Format 4 adds
/// `issue` (2_design.md §11.4); an older line reads with none. Format 5 adds
/// `issue.trigger` and `issue.request_sha256`, for the prompt hook's trigger
/// fires and the writes they are matched to; an older line reads with
/// neither.
pub(crate) const AUDIT_FORMAT: u32 = 5;

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
