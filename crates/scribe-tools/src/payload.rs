//! What a provider sends a hook on stdin (docs/design/scribe/2_design.md
//! §4.1): the fields of each payload scribe uses, and their decoding. The
//! payload is a claim, not authority (STD-05 R1): the hook app holds its
//! `transcript_path` to the transcript provenance check before it acts on a
//! session.

use scribe_store::session::SessionId;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::path::PathBuf;

/// The fields of the `UserPromptSubmit` payload scribe uses; the rest are
/// ignored.
#[derive(Debug, Deserialize)]
pub struct PromptPayload {
    /// The session id.
    pub session_id: String,
    /// The transcript the provider names for the session.
    pub transcript_path: PathBuf,
    /// The working directory.
    pub cwd: String,
    /// The prompt as typed.
    pub prompt: String,
    /// Origin claim, where the harness provides one (`source`,
    /// `prompt_source`/`promptSource`, `origin`, `prompt_origin`/`promptOrigin`,
    /// `turn_origin`/`turnOrigin`); a claim, not authority (STD-05 R1), used
    /// only to refuse agent turns.
    #[serde(default, deserialize_with = "crate::origin::present_value")]
    pub source: Option<serde_json::Value>,
    /// `promptSource`.
    #[serde(
        default,
        rename = "promptSource",
        deserialize_with = "crate::origin::present_value"
    )]
    pub prompt_source_camel: Option<serde_json::Value>,
    /// `prompt_source`.
    #[serde(default, deserialize_with = "crate::origin::present_value")]
    pub prompt_source: Option<serde_json::Value>,
    /// `origin`.
    #[serde(default, deserialize_with = "crate::origin::present_value")]
    pub origin: Option<serde_json::Value>,
    /// `promptOrigin`.
    #[serde(
        default,
        rename = "promptOrigin",
        deserialize_with = "crate::origin::present_value"
    )]
    pub prompt_origin_camel: Option<serde_json::Value>,
    /// `prompt_origin`.
    #[serde(default, deserialize_with = "crate::origin::present_value")]
    pub prompt_origin: Option<serde_json::Value>,
    /// `turnOrigin`.
    #[serde(
        default,
        rename = "turnOrigin",
        deserialize_with = "crate::origin::present_value"
    )]
    pub turn_origin_camel: Option<serde_json::Value>,
    /// `turn_origin`.
    #[serde(default, deserialize_with = "crate::origin::present_value")]
    pub turn_origin: Option<serde_json::Value>,
    /// `isMeta`: a harness turn.
    #[serde(rename = "isMeta", default)]
    pub is_meta: serde_json::Value,
}

/// Whether this payload's turn was written by an agent or the harness,
/// not plainly typed by the user ([`crate::origin::OriginClaims`]
/// over its origin fields, `isMeta`, and prompt). Such a turn records
/// nothing, decides nothing and does not switch `/scribe` on or off.
pub fn agent_originated(payload: &PromptPayload) -> bool {
    crate::origin::prompt_agent_originated(
        &crate::origin::PromptClaims {
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
pub struct SessionStartPayload {
    /// The session id.
    pub session_id: String,
    /// The transcript, when the provider has written one.
    pub transcript_path: Option<PathBuf>,
    /// Why the session started (`startup`, `clear`, …).
    pub source: Option<String>,
    /// Working directory, when the harness sends one. The continuation
    /// reminder matches a recorded session in this directory's project.
    /// Absent, the hook still starts the session and prints no reminder.
    #[serde(default)]
    pub cwd: Option<String>,
}

/// The fields of the `SessionEnd` payload scribe uses.
#[derive(Debug, Deserialize)]
pub struct SessionEndPayload {
    /// The session id.
    pub session_id: String,
}

/// The fields of the `Stop` payload scribe uses. `scribe mark retry` builds
/// one for a session's transcript to catalog its markers as `Stop` would.
#[derive(Debug, Deserialize)]
pub struct StopPayload {
    /// The session id.
    pub session_id: String,
    /// The transcript the provider names for the session.
    pub transcript_path: PathBuf,
    /// Fallback for transcript lines from clients that omit their cwd.
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Parse a payload: the value, or why the bytes are not one.
///
/// # Errors
///
/// The JSON error's message.
pub fn decode<T: DeserializeOwned>(input: &[u8]) -> Result<T, String> {
    serde_json::from_slice(input).map_err(|e| e.to_string())
}

/// The payload's session id, validated.
///
/// # Errors
///
/// The id error's message.
pub fn session(id: &str) -> Result<SessionId, String> {
    id.parse()
        .map_err(|e: scribe_store::session::SessionError| e.to_string())
}
