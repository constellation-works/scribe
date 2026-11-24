//! A hook's stdin, read whole and bounded; the hook app decodes it
//! (docs/design/scribe/2_design.md §4.1).
//!
//! The payload is a claim, not authority (STD-05 R1): any process running as
//! the user can pipe JSON into `scribe hook`. Before a hook acts on a session,
//! the hook app confirms the payload's `transcript_path` is a Claude Code transcript
//! of this user for that session: under the transcripts directory, named
//! `<session_id>.jsonl`, a regular file (not a link) owned by this uid and
//! writable by no group or other user. That stops a confused agent or a
//! mistyped command, not a hostile process of the same user; the design
//! documents it as an accident guard (STD-05 R5).

use scribe_core::CommandError;
use std::io::Read;

/// The largest payload read from stdin. A prompt with pasted logs can be
/// large, but not this large; more is refused rather than buffered without
/// bound.
const MAX_PAYLOAD: u64 = 16 * 1024 * 1024;

/// Read the whole of stdin, up to [`MAX_PAYLOAD`].
pub(super) fn read_stdin() -> Result<Vec<u8>, CommandError> {
    let mut input = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_PAYLOAD + 1)
        .read_to_end(&mut input)
        .map_err(|source| CommandError::Read {
            what: String::from("stdin"),
            source,
        })?;
    if u64::try_from(input.len()).unwrap_or(u64::MAX) > MAX_PAYLOAD {
        return Err(CommandError::HookPayload(format!(
            "stdin is larger than {} MiB",
            MAX_PAYLOAD / 1024 / 1024
        )));
    }
    Ok(input)
}
