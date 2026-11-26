//! The origin guard (docs/design/scribe/2_design.md §4.1): whether a user
//! turn, or one Claude Code transcript line, was written by an agent or the
//! harness rather than plainly typed by Daniel. Transcript parsing and
//! `#scribe` capture both apply it; it is re-exported from
//! scribe-core's `capture`, which first owned it. Pure functions over text and
//! JSON values; no I/O (STD-02 R5).

use serde::Deserialize;

/// Origin values that mean Daniel typed the prompt himself.
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

/// Origin claims read from a Claude Code transcript line or hook payload
/// (docs/design/scribe/2_design.md §4.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OriginClaims {
    invalid: bool,
    prompt_sources: Vec<String>,
    /// Every `origin.kind` the line carries. A queued message has one on the
    /// line and one on its attachment, and each must be plain.
    origin_kinds: Vec<String>,
    turn_origins: Vec<String>,
    other: Vec<String>,
}

impl OriginClaims {
    /// Record an origin claim (`source`, `promptOrigin`/`prompt_origin`) that
    /// must be a string when present.
    pub fn push_claim(&mut self, value: Option<&serde_json::Value>) {
        let Some(value) = value else { return };
        match value.as_str() {
            Some(s) => self.other.push(s.to_owned()),
            None => self.invalid = true,
        }
    }

    /// Record `promptSource`/`prompt_source`, which must be a string when
    /// present; `"sdk"` is treated as plain only alongside a plain `turnOrigin`
    /// or `origin.kind`.
    pub fn push_prompt_source(&mut self, value: Option<&serde_json::Value>) {
        let Some(value) = value else { return };
        match value.as_str() {
            Some(s) => self.prompt_sources.push(s.to_owned()),
            None => self.invalid = true,
        }
    }

    /// Record `origin`, which may be a string or an object with a non-empty
    /// string `kind`.
    pub fn push_origin(&mut self, value: Option<&serde_json::Value>) {
        let Some(value) = value else { return };
        match value {
            serde_json::Value::String(s) => self.other.push(s.clone()),
            serde_json::Value::Object(map) => {
                match map.get("kind").and_then(serde_json::Value::as_str) {
                    Some(kind) if !kind.trim().is_empty() => {
                        self.origin_kinds.push(kind.to_owned());
                    }
                    _ => self.invalid = true,
                }
            }
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::Array(_) => self.invalid = true,
        }
    }

    /// Record `turnOrigin`/`turn_origin`, which must be a string when present.
    pub fn push_turn_origin(&mut self, value: Option<&serde_json::Value>) {
        let Some(value) = value else { return };
        match value.as_str() {
            Some(s) => self.turn_origins.push(s.to_owned()),
            None => self.invalid = true,
        }
    }

    /// Whether the origin claims alone refuse the turn (unexpected shape,
    /// non-plain origin value, or `promptSource: sdk` without a plain
    /// `turnOrigin` or `origin.kind`).
    #[must_use]
    pub fn claims_agent_originated(&self) -> bool {
        if self.invalid {
            return true;
        }
        let desktop_plain = self.origin_kinds.iter().any(|k| is_explicitly_plain(k))
            || self.turn_origins.iter().any(|s| is_explicitly_plain(s));
        if self.other.iter().any(|s| !is_plain_or_empty(s))
            || self.turn_origins.iter().any(|s| !is_plain_or_empty(s))
            || self.origin_kinds.iter().any(|k| !is_explicitly_plain(k))
        {
            return true;
        }
        self.prompt_sources.iter().any(|s| {
            let norm = normalize_origin(s);
            if norm == "sdk" {
                !desktop_plain
            } else {
                !norm.is_empty() && !PLAIN_SOURCES.contains(&norm.as_str())
            }
        })
    }

    /// Whether the claims or `prompt` text mark the turn as agent-originated.
    #[must_use]
    pub fn is_agent_originated(&self, prompt: &str) -> bool {
        self.claims_agent_originated() || agent_originated(&[], prompt)
    }
}

/// The origin claims a hook prompt payload carries (§4.1): `isMeta` and each
/// origin field, a present JSON value or `None`. A claim is a claim, not
/// authority (STD-05 R1), used only to refuse agent turns.
#[derive(Clone, Copy, Debug, Default)]
pub struct PromptClaims<'a> {
    /// `isMeta` is `true`.
    pub is_meta: bool,
    /// `source`.
    pub source: Option<&'a serde_json::Value>,
    /// `promptSource`.
    pub prompt_source_camel: Option<&'a serde_json::Value>,
    /// `prompt_source`.
    pub prompt_source: Option<&'a serde_json::Value>,
    /// `origin`.
    pub origin: Option<&'a serde_json::Value>,
    /// `promptOrigin`.
    pub prompt_origin_camel: Option<&'a serde_json::Value>,
    /// `prompt_origin`.
    pub prompt_origin: Option<&'a serde_json::Value>,
    /// `turnOrigin`.
    pub turn_origin_camel: Option<&'a serde_json::Value>,
    /// `turn_origin`.
    pub turn_origin: Option<&'a serde_json::Value>,
}

/// The fields of one Claude Code transcript line the origin guard reads beside
/// its text (§4.1): the line's kind, uuid and entry point, and every origin
/// claim it carries. A claim field keeps an explicit JSON `null` as present,
/// so the guard refuses it as an unexpected shape rather than reading it as
/// absent.
#[derive(Deserialize)]
pub struct LineMeta {
    /// The line's `type`.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    /// The line's `uuid`.
    #[serde(default)]
    pub uuid: Option<serde_json::Value>,
    /// The line's `entrypoint`.
    #[serde(default)]
    pub entrypoint: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "present_value")]
    source: Option<serde_json::Value>,
    #[serde(default, rename = "promptSource", deserialize_with = "present_value")]
    prompt_source_camel: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "present_value")]
    prompt_source: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "present_value")]
    origin: Option<serde_json::Value>,
    #[serde(default, rename = "promptOrigin", deserialize_with = "present_value")]
    prompt_origin_camel: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "present_value")]
    prompt_origin: Option<serde_json::Value>,
    #[serde(default, rename = "turnOrigin", deserialize_with = "present_value")]
    turn_origin_camel: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "present_value")]
    turn_origin: Option<serde_json::Value>,
    /// A queued message's attachment, whose origin is a claim as well.
    #[serde(default)]
    attachment: Option<LineAttachment>,
}

impl LineMeta {
    /// Every origin claim the line carries: its own fields and, on a queued
    /// message's `attachment` line, the attachment's origin. Each is checked,
    /// so a claim that contradicts the others refuses the line wherever it
    /// sits. An ordinary row is corroborated by its own fields alone: an
    /// attachment on it is no claim on it.
    #[must_use]
    pub fn origin_claims(&self) -> OriginClaims {
        let mut origin = OriginClaims::default();
        origin.push_claim(self.source.as_ref());
        origin.push_prompt_source(self.prompt_source_camel.as_ref());
        origin.push_prompt_source(self.prompt_source.as_ref());
        origin.push_origin(self.origin.as_ref());
        if self.kind.as_deref() == Some("attachment") {
            origin.push_origin(self.attachment.as_ref().and_then(|a| a.origin.as_ref()));
        }
        origin.push_claim(self.prompt_origin_camel.as_ref());
        origin.push_claim(self.prompt_origin.as_ref());
        origin.push_turn_origin(self.turn_origin_camel.as_ref());
        origin.push_turn_origin(self.turn_origin.as_ref());
        origin
    }
}

#[derive(Deserialize)]
struct LineAttachment {
    #[serde(default, deserialize_with = "present_value")]
    origin: Option<serde_json::Value>,
}

fn normalize_origin(source: &str) -> String {
    source.trim().to_lowercase().replace(['_', ' '], "-")
}

fn is_explicitly_plain(source: &str) -> bool {
    let normalized = normalize_origin(source);
    !normalized.is_empty() && PLAIN_SOURCES.contains(&normalized.as_str())
}

fn is_plain_or_empty(source: &str) -> bool {
    let normalized = normalize_origin(source);
    normalized.is_empty() || PLAIN_SOURCES.contains(&normalized.as_str())
}

/// Serde helper that distinguishes an explicit `null` from an omitted field,
/// so fail-closed origin checks refuse `"origin": null` as an unexpected shape.
///
/// # Errors
///
/// Propagates deserialization errors from the underlying JSON value.
pub fn present_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error> {
    serde::Deserialize::deserialize(deserializer).map(Some)
}

/// Whether a hook prompt with these `claims` was written by an agent or the
/// harness rather than plainly typed: `isMeta`, the claims
/// ([`OriginClaims`]), or the prompt text. The one rule for the prompt hook
/// and for a Codex turn replayed through it.
#[must_use]
pub fn prompt_agent_originated(claims: &PromptClaims<'_>, prompt: &str) -> bool {
    if claims.is_meta {
        return true;
    }
    let mut origin = OriginClaims::default();
    origin.push_claim(claims.source);
    origin.push_prompt_source(claims.prompt_source_camel);
    origin.push_prompt_source(claims.prompt_source);
    origin.push_origin(claims.origin);
    origin.push_claim(claims.prompt_origin_camel);
    origin.push_claim(claims.prompt_origin);
    origin.push_turn_origin(claims.turn_origin_camel);
    origin.push_turn_origin(claims.turn_origin);
    origin.is_agent_originated(prompt)
}

/// Whether the origin claims on one Claude Code line refuse it. A line whose
/// origin fields do not parse is refused too: its shape is one the guard does
/// not know.
pub(crate) fn line_refused(line: &[u8]) -> bool {
    match serde_json::from_slice::<LineMeta>(line) {
        Ok(meta) => meta.origin_claims().claims_agent_originated(),
        Err(_) => true,
    }
}

/// The origin guard (docs/design/scribe/2_design.md §4.1): whether a user
/// turn was written by an agent or the harness, not plainly typed by
/// Daniel. `origins` are the turn's origin claims where the harness gives
/// them (`source`, `prompt_source`, `origin`, `prompt_origin`, `turn_origin`);
/// `prompt` is its text. A non-plain origin value, an agent wrapper anywhere in
/// the prompt, or a prompt opening with a markup tag (a plainly typed prompt
/// does not open that way) is not his. An accident guard (STD-05 R5): a
/// process running as Daniel can still forge a plain turn.
#[must_use]
pub fn agent_originated(origins: &[&str], prompt: &str) -> bool {
    if origins.iter().any(|source| !is_plain_or_empty(source)) {
        return true;
    }
    // Underscores and hyphens spell the same wrapper (`<agent_message>`).
    let lower = prompt.to_lowercase().replace('_', "-");
    if AGENT_WRAPPERS.iter().any(|tag| lower.contains(tag)) {
        return true;
    }
    let trimmed = lower.trim_start_matches('\u{feff}').trim_start();
    trimmed
        .strip_prefix('<')
        .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
}
