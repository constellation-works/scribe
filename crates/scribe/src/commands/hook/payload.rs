//! What Claude Code sends a hook on stdin, and the transcript provenance
//! check (docs/design/scribe/2_design.md §4.1), read through
//! `scribe_tools::transcript` over this context's roots and provider.
//!
//! The payload is a claim, not authority (STD-05 R1): any process running as
//! the user can pipe JSON into `scribe hook`. Before a hook acts on a session,
//! it confirms the payload's `transcript_path` is a Claude Code transcript
//! of this user for that session: under the transcripts directory, named
//! `<session_id>.jsonl`, a regular file (not a link) owned by this uid and
//! writable by no group or other user. That stops a confused agent or a
//! mistyped command, not a hostile process of the same user; the design
//! documents it as an accident guard (STD-05 R5).

use crate::error::CliError;
use scribe_core::SessionId;
/// What the provenance check found, and why [`read_checked`] gave no bytes
/// for a transcript that exists: the core port's types.
pub(in crate::commands) use scribe_core::runtime::{TranscriptState as Transcript, Unreadable};
use scribe_tools::transcript::{self, DiscoverError, Scope};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::io::Read;
use std::path::{Path, PathBuf};

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
    /// `prompt_source`/`promptSource`, `origin`, `prompt_origin`/`promptOrigin`,
    /// `turn_origin`/`turnOrigin`); a claim, not authority (STD-05 R1), used
    /// only to refuse agent turns.
    #[serde(default, deserialize_with = "scribe_core::capture::present_value")]
    pub(super) source: Option<serde_json::Value>,
    #[serde(
        default,
        rename = "promptSource",
        deserialize_with = "scribe_core::capture::present_value"
    )]
    pub(super) prompt_source_camel: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "scribe_core::capture::present_value")]
    pub(super) prompt_source: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "scribe_core::capture::present_value")]
    pub(super) origin: Option<serde_json::Value>,
    #[serde(
        default,
        rename = "promptOrigin",
        deserialize_with = "scribe_core::capture::present_value"
    )]
    pub(super) prompt_origin_camel: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "scribe_core::capture::present_value")]
    pub(super) prompt_origin: Option<serde_json::Value>,
    #[serde(
        default,
        rename = "turnOrigin",
        deserialize_with = "scribe_core::capture::present_value"
    )]
    pub(super) turn_origin_camel: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "scribe_core::capture::present_value")]
    pub(super) turn_origin: Option<serde_json::Value>,
    #[serde(rename = "isMeta", default)]
    pub(super) is_meta: serde_json::Value,
}

/// Whether this payload's turn was written by an agent or the harness,
/// not plainly typed by the user ([`scribe_core::capture::OriginClaims`]
/// over its origin fields, `isMeta`, and prompt). Such a turn records
/// nothing, decides nothing and does not switch `/scribe` on or off.
pub(super) fn agent_originated(payload: &PromptPayload) -> bool {
    scribe_core::capture::prompt_agent_originated(
        &scribe_core::capture::PromptClaims {
            is_meta: payload.is_meta.as_bool() == Some(true),
            source: payload.source.as_ref(),
            prompt_source_camel: payload.prompt_source_camel.as_ref(),
            prompt_source: payload.prompt_source.as_ref(),
            origin: payload.origin.as_ref(),
            prompt_origin_camel: payload.prompt_origin_camel.as_ref(),
            prompt_origin: payload.prompt_origin.as_ref(),
            turn_origin_camel: payload.turn_origin_camel.as_ref(),
            turn_origin: payload.turn_origin.as_ref(),
        },
        &payload.prompt,
    )
}

/// The fields of the `SessionStart` payload scribe uses.
#[derive(Debug, Deserialize)]
pub(super) struct SessionStartPayload {
    pub(super) session_id: String,
    pub(super) transcript_path: Option<PathBuf>,
    pub(super) source: Option<String>,
    /// Working directory, when the harness sends one. The continuation
    /// reminder matches a recorded session in this directory's project.
    /// Absent, the hook still starts the session and prints no reminder.
    #[serde(default)]
    pub(super) cwd: Option<String>,
}

/// The fields of the `SessionEnd` payload scribe uses.
#[derive(Debug, Deserialize)]
pub(super) struct SessionEndPayload {
    pub(super) session_id: String,
}

/// The fields of the `Stop` payload scribe uses. `scribe mark retry` builds
/// one for a session's transcript to catalog its markers as `Stop` would.
#[derive(Debug, Deserialize)]
pub(in crate::commands) struct StopPayload {
    pub(in crate::commands) session_id: String,
    pub(in crate::commands) transcript_path: PathBuf,
    /// Fallback for transcript lines from clients that omit their cwd.
    #[serde(default)]
    pub(in crate::commands) cwd: Option<String>,
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

/// The transcript roots and provider `ctx` reads as.
fn scope(ctx: &crate::app::Context) -> Scope<'_> {
    Scope::new(ctx.transcript_roots(), ctx.provider())
}

/// A listing failure as the CLI error it was before the move.
fn listing(err: DiscoverError) -> CliError {
    match err {
        DiscoverError::Read { what, source } => CliError::Read { what, source },
        DiscoverError::Refused(why) => CliError::Usage(why),
    }
}

/// Provider-aware check ([`transcript::check`]).
pub(in crate::commands) fn check(
    ctx: &crate::app::Context,
    path: &Path,
    session: &SessionId,
) -> Result<Transcript, String> {
    transcript::check(&scope(ctx), path, session)
}

/// Check and read through one handle ([`transcript::read_checked`]).
pub(in crate::commands) fn read_checked(
    ctx: &crate::app::Context,
    path: &Path,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, Unreadable> {
    transcript::read_checked(&scope(ctx), path, session)
}

/// [`read_checked`] without its Codex rollout check
/// ([`transcript::read_checked_bytes`]).
pub(in crate::commands) fn read_checked_bytes(
    ctx: &crate::app::Context,
    path: &Path,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, Unreadable> {
    transcript::read_checked_bytes(&scope(ctx), path, session)
}

/// Read only the transcript prelude ([`transcript::claude_header`]).
pub(in crate::commands) fn claude_header(
    ctx: &crate::app::Context,
    path: &Path,
    session: &SessionId,
) -> Result<Option<Vec<u8>>, Unreadable> {
    transcript::claude_header(&scope(ctx), path, session)
}

/// Open and check one file before any read or seek
/// ([`transcript::open_checked`]), for the read tests.
#[cfg(test)]
pub(super) fn open_checked(
    ctx: &crate::app::Context,
    path: &Path,
    session: &SessionId,
) -> Result<Option<(scribe_tools::fs::File, scribe_tools::fs::Metadata)>, Unreadable> {
    transcript::open_checked(&scope(ctx), path, session)
}

/// Read a transcript [`check`] passed ([`transcript::read_for`]).
pub(in crate::commands) fn read_for(
    ctx: &crate::app::Context,
    path: &Path,
) -> std::io::Result<Vec<u8>> {
    transcript::read_for(&scope(ctx), path)
}

/// The active provider's live and archived rollouts
/// ([`transcript::files_for`]).
pub(in crate::commands) fn files_for(
    ctx: &crate::app::Context,
) -> Result<Vec<(SessionId, PathBuf)>, CliError> {
    transcript::files_for(&scope(ctx)).map_err(listing)
}

/// [`files_for`], with the refused entries of every root
/// ([`transcript::discovered_for`]).
pub(in crate::commands) fn discovered_for(
    ctx: &crate::app::Context,
) -> Result<scribe_core::runtime::Discovered, CliError> {
    transcript::discovered_for(&scope(ctx)).map_err(listing)
}

/// The transcript of `id` alone ([`transcript::locate_own`]).
pub(in crate::commands) fn locate_own(
    ctx: &crate::app::Context,
    id: &SessionId,
) -> Result<Option<PathBuf>, CliError> {
    transcript::locate_own(&scope(ctx), id).map_err(listing)
}

/// The transcripts port's Claude window ([`transcript::claude_window`]).
#[cfg(unix)]
pub(in crate::commands) fn claude_window(
    ctx: &crate::app::Context,
    path: &Path,
    session: &SessionId,
    prior: Option<&scribe_core::ClaudeCapturePosition>,
) -> Result<Option<scribe_core::runtime::ClaudeRead>, Unreadable> {
    transcript::claude_window(&scope(ctx), path, session, prior)
}
