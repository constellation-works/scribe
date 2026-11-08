//! What Claude Code sends a hook on stdin, and the transcript provenance
//! check (docs/design/scribe/2_design.md §4.1).
//!
//! The payload is a claim, not authority (STD-05 R1): any process running as
//! Daniel can pipe JSON into `scribe hook`. Before a hook acts on a session,
//! it confirms the payload's `transcript_path` is a Claude Code transcript
//! of this user for that session: under the transcripts directory, named
//! `<session_id>.jsonl`, a regular file (not a link) owned by this uid. That
//! stops a confused agent or a mistyped command, not a hostile process of the
//! same user; the design documents it as an accident guard (STD-05 R5).

use crate::error::CliError;
use scribe_core::SessionId;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// The largest payload read from stdin. A prompt with pasted logs can be
/// large, but not this large; more is refused rather than buffered without
/// bound.
const MAX_PAYLOAD: u64 = 16 * 1024 * 1024;

/// The fields of the `UserPromptSubmit` payload scribe uses; the rest are
/// ignored.
#[derive(Debug, Deserialize)]
pub(super) struct PromptPayload {
    pub(super) session_id: String,
    pub(super) transcript_path: PathBuf,
    pub(super) cwd: String,
    pub(super) prompt: String,
    /// Origin claim, where the harness provides one (`source`,
    /// `prompt_source`, `origin` or `prompt_origin`); a claim, not
    /// authority (STD-05 R1), used only to refuse agent turns.
    #[serde(default)]
    pub(super) source: Option<String>,
    #[serde(default, alias = "promptSource")]
    pub(super) prompt_source: Option<String>,
    #[serde(default)]
    pub(super) origin: Option<String>,
    #[serde(default, alias = "promptOrigin")]
    pub(super) prompt_origin: Option<String>,
}

/// Source values that mean Daniel typed the prompt himself.
const PLAIN_SOURCES: &[&str] = &[
    "typed",
    "user",
    "human",
    "daniel",
    "interactive",
    "keyboard",
    "manual",
    "direct",
    "user-typed",
    "plain",
];

/// Opening tags of agent- or system-written user turns. A prompt carrying
/// one of these anywhere is not Daniel's typing, however a `#scribe` line
/// sits inside it.
const AGENT_WRAPPERS: &[&str] = &[
    "<agent-message",
    "<task-notification",
    "<side-sessions-event",
    "<side-session-event",
    "<side-session",
    "<scheduled-prompt",
    "<scheduled-loop",
    "<scheduled",
    "<loop-prompt",
    "<loop-",
    "<teammate-message",
    "<cross-session",
];

/// Whether this payload's turn was written by an agent or the harness,
/// not plainly typed by Daniel: a non-plain origin field, an agent
/// wrapper anywhere in the prompt, or a prompt opening with a markup tag
/// (a plainly typed prompt does not open that way). Such a turn records
/// nothing, decides nothing and does not switch `/scribe` on or off.
/// This is an accident guard like the transcript check (STD-05 R5): a
/// process running as Daniel can still forge a plain payload.
pub(super) fn agent_originated(payload: &PromptPayload) -> bool {
    let sources = [
        payload.source.as_deref(),
        payload.prompt_source.as_deref(),
        payload.origin.as_deref(),
        payload.prompt_origin.as_deref(),
    ];
    for source in sources.into_iter().flatten() {
        let normalized = source.trim().to_lowercase().replace(['_', ' '], "-");
        if !normalized.is_empty() && !PLAIN_SOURCES.contains(&normalized.as_str()) {
            return true;
        }
    }
    // Underscores and hyphens spell the same wrapper (`<agent_message>`).
    let lower = payload.prompt.to_lowercase().replace('_', "-");
    if AGENT_WRAPPERS.iter().any(|tag| lower.contains(tag)) {
        return true;
    }
    let trimmed = lower.trim_start_matches('\u{feff}').trim_start();
    trimmed
        .strip_prefix('<')
        .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
}

/// The fields of the `SessionStart` payload scribe uses.
#[derive(Debug, Deserialize)]
pub(super) struct SessionStartPayload {
    pub(super) session_id: String,
}

/// The fields of the `SessionEnd` payload scribe uses.
#[derive(Debug, Deserialize)]
pub(super) struct SessionEndPayload {
    pub(super) session_id: String,
}

/// The fields of the `Stop` payload scribe uses.
#[derive(Debug, Deserialize)]
pub(super) struct StopPayload {
    pub(super) session_id: String,
    pub(super) transcript_path: PathBuf,
}

/// Read the whole of stdin, up to [`MAX_PAYLOAD`].
pub(super) fn read_stdin() -> Result<Vec<u8>, CliError> {
    let mut input = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_PAYLOAD + 1)
        .read_to_end(&mut input)
        .map_err(|source| CliError::Read {
            what: String::from("stdin"),
            source,
        })?;
    if u64::try_from(input.len()).unwrap_or(u64::MAX) > MAX_PAYLOAD {
        return Err(CliError::HookPayload(format!(
            "stdin is larger than {} MiB",
            MAX_PAYLOAD / 1024 / 1024
        )));
    }
    Ok(input)
}

/// Parse a payload.
pub(super) fn decode<T: DeserializeOwned>(input: &[u8]) -> Result<T, CliError> {
    serde_json::from_slice(input).map_err(|e| CliError::HookPayload(e.to_string()))
}

/// The payload's session id, validated.
pub(super) fn session(id: &str) -> Result<SessionId, CliError> {
    id.parse()
        .map_err(|e: scribe_core::SessionError| CliError::HookPayload(e.to_string()))
}

/// What the provenance check found.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::commands) enum Transcript {
    /// The transcript exists and passed.
    Present,
    /// The path is right but there is no file yet (a session's first prompt
    /// can arrive before Claude Code writes its transcript).
    Missing,
}

/// Check `path` is this user's Claude Code transcript for `session`, under
/// `root` (the transcripts directory). The error is why not, for the reply.
pub(in crate::commands) fn check_transcript(
    path: &Path,
    session: &SessionId,
    root: Option<&Path>,
) -> Result<Transcript, String> {
    let root = root.ok_or("no transcripts directory: HOME is not set")?;
    let inside = path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        && path.starts_with(root);
    if !inside {
        return Err(format!(
            "the transcript path is not under {}",
            root.display()
        ));
    }
    let named = path.extension().is_some_and(|e| e == "jsonl")
        && path
            .file_stem()
            .is_some_and(|stem| stem == session.as_str());
    if !named {
        return Err(String::from(
            "the transcript is not named for the payload's session_id",
        ));
    }
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Transcript::Missing),
        Err(e) => return Err(format!("cannot inspect the transcript: {e}")),
    };
    if !meta.file_type().is_file() {
        return Err(String::from("the transcript is not a regular file"));
    }
    owned_by_this_user(&meta)?;
    Ok(Transcript::Present)
}

#[cfg(unix)]
fn owned_by_this_user(meta: &std::fs::Metadata) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let uid = rustix::process::geteuid().as_raw();
    if meta.uid() == uid {
        Ok(())
    } else {
        Err(format!(
            "the transcript is owned by uid {}, not this user (uid {uid})",
            meta.uid()
        ))
    }
}

#[cfg(not(unix))]
fn owned_by_this_user(_meta: &std::fs::Metadata) -> Result<(), String> {
    Ok(())
}
