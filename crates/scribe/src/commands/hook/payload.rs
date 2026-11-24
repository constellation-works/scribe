//! What Claude Code sends a hook on stdin (docs/design/scribe/2_design.md
//! §4.1). The transcript provenance check it is held to is
//! `scribe_core::runtime::host`'s, over `scribe_tools::transcript`.
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
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::io::Read;
use std::path::PathBuf;

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
