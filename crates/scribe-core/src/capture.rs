//! `#scribe` lines: finding them in a typed turn, and the model-free
//! settling commands (docs/design/scribe/2_design.md §4.1, §4.2, §14).
//!
//! [`lines`] reads a prompt line by line. A line is a `#scribe` line when it
//! starts with `#scribe` (after optional leading whitespace) and is not inside
//! a code fence, a blockquote or a `<pasted_content>` block, so a pasted log
//! or a quoted message that happens to contain `#scribe` never acts. A bare
//! `#decision` or any other hashtag is not one.
//!
//! Since §14 step 2 a `#scribe` line records nothing here: like a marker
//! inside a sentence it marks the turn, and the same turn's `Stop` catalogs
//! it with a statement and his quoted words (`crate::annotation`). The
//! grammar of forms 1 to 3 and the referent call are gone. Only form 4 is
//! parsed here, a whole line, model-free:
//!
//! 4. `#scribe approve|reject <p-id>` · `#scribe retire|forget <entry-id>` ·
//!    `#scribe forget <session-id>` (`s-<n>`, or the provider's UUID; §15.7)
//!
//! It parses to a [`Settling`] ([`crate::consistency::settle`] applies it).
//! A settling verb with the wrong kind of id is refused,
//! never read as a marker, and so is [`GLOBAL_OPENER`] on a settling line.
//!
//! An entry under an action tag also keeps an [`action_window`]: the turns
//! since the previous `#scribe` line of the session, capped by
//! [`WINDOW_TURNS`] and [`WINDOW_CHARS`] and cut from the oldest turn (§4.6).
//! It takes no model.
//!
//! Pure functions over text: no I/O (STD-02 R5).

use crate::consistency::Settle;
use crate::entry::{Cite, EntryId, Speaker, Window, WindowTurn};
use crate::proposal::ProposalId;
use crate::record::{Label, Project};
use crate::session::SessionId;
use crate::transcript::Message;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fmt::Write as _;
use time::OffsetDateTime;

/// The marker that opens a `#scribe` line, or marks a turn anywhere in a
/// sentence (§14.1).
pub const MARKER: &str = "#scribe";

/// The opener that records the entry with `project: global` instead of the
/// cwd's project, in place of [`MARKER`]. It is not a topic hashtag and not
/// a tag.
pub const GLOBAL_OPENER: &str = "#scribe/global";

/// Verbs that settle a proposal or hide an entry (form 4).
pub(crate) const SETTLING: [&str; 4] = ["approve", "reject", "retire", "forget"];

/// The most turns an action entry's window keeps (§4.6).
pub const WINDOW_TURNS: usize = 40;
/// The most characters of turn text an action entry's window keeps, all
/// turns together (§4.6).
pub const WINDOW_CHARS: usize = 12_000;
/// The most characters one turn of a window keeps; a longer turn keeps its
/// first this many and is marked `cut`.
pub const WINDOW_TURN_CHARS: usize = 4_000;

/// Why a settling line was not applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A settling verb whose id is not the kind it takes.
    BadId {
        /// The verb as typed, lowercased.
        verb: String,
        /// What it takes, for the reply.
        expected: &'static str,
    },
    /// [`GLOBAL_OPENER`] on a settling line, which records no entry.
    MarkerNotOnEntry,
}

impl Refusal {
    /// The stable code the audit log and the reply carry (STD-01 R19).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadId { .. } | Self::MarkerNotOnEntry => "unparsed_instruction",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadId { verb, expected } => write!(f, "#scribe {verb} takes {expected}"),
            Self::MarkerNotOnEntry => write!(
                f,
                "{GLOBAL_OPENER} records an entry for every repository; a settling line takes #scribe"
            ),
        }
    }
}

/// A settling line that was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// The line's number within the prompt, from 1.
    pub line: u32,
    /// The whole line exactly as typed.
    pub said: String,
    /// Why.
    pub reason: Refusal,
}

impl Refused {
    /// The text the hook echoes.
    #[must_use]
    pub fn message(&self) -> String {
        self.reason.to_string()
    }
}

/// A form-4 line: settle a proposal, or hide or blank an entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settling {
    /// The line's number within the prompt, from 1.
    pub line: u32,
    /// The whole line exactly as typed.
    pub said: String,
    /// What it does.
    pub action: Settle,
}

/// The result for one settling line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    /// It settles a proposal or changes an entry's status (form 4).
    Settle(Settling),
    /// It was refused.
    Refused(Refused),
}

impl Parsed {
    /// The line's number within the prompt.
    #[must_use]
    pub fn line(&self) -> u32 {
        match self {
            Self::Settle(s) => s.line,
            Self::Refused(r) => r.line,
        }
    }
}

/// Every settling line of `prompt` (form 4), in order, parsed. A `#scribe`
/// line that does not open with a settling verb is a marker, not a command,
/// and is not here (`crate::annotation::marks` has it).
#[must_use]
pub fn scan(prompt: &str) -> Vec<Parsed> {
    lines(prompt)
        .into_iter()
        .filter_map(|(line, said)| parse(line, said))
        .collect()
}

/// Whether `said`, a `#scribe` line, is a settling command (form 4) rather
/// than a marker.
#[must_use]
pub fn settles(said: &str) -> bool {
    parse(0, said).is_some()
}

/// The instruction lines of `prompt`, unparsed: `(number from 1, the line
/// as typed)`. Lines in code fences, blockquotes and `<pasted_content>`
/// blocks are skipped. Empty for a prompt with no `#scribe` line, which
/// is most of them.
#[must_use]
pub fn lines(prompt: &str) -> Vec<(u32, &str)> {
    plain_lines(prompt)
        .into_iter()
        .filter(|(_, line)| is_instruction(line.trim_start()))
        .collect()
}

/// The lines of `prompt` that are typed text: `(number from 1, the line
/// without its `\r`)`, every line inside a code fence (and the fence lines
/// themselves), a blockquote line and a `<pasted_content>` block left out.
#[must_use]
pub fn plain_lines(prompt: &str) -> Vec<(u32, &str)> {
    let mut found = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut pasted = false;
    for (index, raw) in prompt.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = line.trim_start();
        if pasted {
            if line.contains("</pasted_content>") {
                pasted = false;
            }
            continue;
        }
        if let Some((mark, len)) = fence {
            if fence_marker(trimmed).is_some_and(|(m, l)| m == mark && l >= len)
                && trimmed.trim_start_matches(mark).trim().is_empty()
            {
                fence = None;
            }
            continue;
        }
        if let Some(open) = fence_marker(trimmed) {
            fence = Some(open);
            continue;
        }
        if let Some(at) = line.find("<pasted_content") {
            pasted = !line[at..].contains("</pasted_content>");
            continue;
        }
        if trimmed.starts_with('>') {
            continue;
        }
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        found.push((number, line));
    }
    found
}

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

/// The fields of one Claude Code transcript line the origin guard reads beside
/// its text (§4.1): the line's kind, uuid and entry point, and every origin
/// claim it carries. A claim field keeps an explicit JSON `null` as present,
/// so the guard refuses it as an unexpected shape rather than reading it as
/// absent.
#[derive(Deserialize)]
pub(crate) struct LineMeta {
    #[serde(rename = "type", default)]
    pub(crate) kind: Option<String>,
    #[serde(default)]
    pub(crate) uuid: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) entrypoint: Option<serde_json::Value>,
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

#[derive(Deserialize)]
struct LineAttachment {
    #[serde(default, deserialize_with = "present_value")]
    origin: Option<serde_json::Value>,
}

impl LineMeta {
    /// Every origin claim the line carries: its own fields, then a queued
    /// message's attachment origin. Each is checked, so a claim that
    /// contradicts the others refuses the line wherever it sits.
    pub(crate) fn origin_claims(&self) -> OriginClaims {
        let mut origin = OriginClaims::default();
        origin.push_claim(self.source.as_ref());
        origin.push_prompt_source(self.prompt_source_camel.as_ref());
        origin.push_prompt_source(self.prompt_source.as_ref());
        origin.push_origin(self.origin.as_ref());
        origin.push_origin(self.attachment.as_ref().and_then(|a| a.origin.as_ref()));
        origin.push_claim(self.prompt_origin_camel.as_ref());
        origin.push_claim(self.prompt_origin.as_ref());
        origin.push_turn_origin(self.turn_origin_camel.as_ref());
        origin.push_turn_origin(self.turn_origin.as_ref());
        origin
    }
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

/// A fence opener or closer: three or more backticks or tildes.
fn fence_marker(trimmed: &str) -> Option<(char, usize)> {
    let mark = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = trimmed.chars().take_while(|&c| c == mark).count();
    (len >= 3).then_some((mark, len))
}

fn is_instruction(trimmed: &str) -> bool {
    // A `#scribe/` line is an attempted global opener; a malformed one is
    // refused as a marker (`crate::annotation::marks`), never ignored.
    if trimmed.starts_with("#scribe/") {
        return true;
    }
    trimmed
        .strip_prefix(MARKER)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

fn is(word: &str, set: &[&str]) -> bool {
    let lower = word.to_lowercase();
    set.contains(&lower.as_str())
}

/// Parse one `#scribe` line as a settling command; `None` when its first
/// word after the opener is not a settling verb (it is a marker).
fn parse(line: u32, said: &str) -> Option<Parsed> {
    let all = words(said.trim_start());
    let opener = *all.first()?;
    let global = opener == GLOBAL_OPENER;
    if opener != MARKER && !global {
        return None;
    }
    let mut body: Vec<&str> = all.iter().skip(1).copied().collect();
    let first = *body.first()?;
    if !is(first, &SETTLING) {
        return None;
    }
    // Trailing hashtags are not part of the command.
    while body.len() > 1 && body.last().is_some_and(|w| w.starts_with('#')) {
        body.pop();
    }
    let refuse = |reason| {
        Some(Parsed::Refused(Refused {
            line,
            said: said.to_owned(),
            reason,
        }))
    };
    if global {
        return refuse(Refusal::MarkerNotOnEntry);
    }
    let verb = first.to_lowercase();
    let id = match body.as_slice() {
        [_, id] => id.trim_end_matches(['.', '!', ',', ';']),
        _ => "",
    };
    let action = match verb.as_str() {
        "approve" | "reject" => id.parse::<ProposalId>().ok().map(|p| {
            if verb == "approve" {
                Settle::Approve(p)
            } else {
                Settle::Reject(p)
            }
        }),
        "forget" if session_id(id).is_some() => session_id(id).map(Settle::ForgetSession),
        _ => id.parse::<EntryId>().ok().map(|e| {
            if verb == "retire" {
                Settle::Retire(e)
            } else {
                Settle::Forget(e)
            }
        }),
    };
    match action {
        Some(action) => Some(Parsed::Settle(Settling {
            line,
            said: said.to_owned(),
            action,
        })),
        None => {
            let expected = match verb.as_str() {
                "approve" | "reject" => "a proposal id, such as p-4",
                "forget" => {
                    "an entry id, such as preferences-12, or a session id, such as s-4 or 7c2b9d14"
                }
                _ => "an entry id, such as preferences-12",
            };
            refuse(Refusal::BadId { verb, expected })
        }
    }
}

/// The group lengths of a UUID, which a Claude Code or Codex session id is.
const UUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

/// The session a forget names: `s-<n>` (no leading zero), a provider
/// session id in UUID form, or a prefix of one (at least
/// [`crate::session::MIN_ID_PREFIX`] characters, as a table prints it). A UUID or
/// its prefix is read as a session before an entry id, which it could
/// otherwise swallow when its last group is all digits. The store resolves a
/// prefix, refusing an ambiguous one. Anything else is an entry id or
/// nothing.
fn session_id(id: &str) -> Option<SessionId> {
    let numbered = id.strip_prefix("s-").is_some_and(|n| {
        !n.is_empty() && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit())
    });
    (numbered || is_uuid_prefix(id))
        .then(|| id.parse().ok())
        .flatten()
}

/// Whether `id` is a UUID, whole or cut anywhere after its first
/// [`crate::session::MIN_ID_PREFIX`] characters: hex groups of 8-4-4-4-12, every
/// group but the last complete and the last neither empty nor too long.
fn is_uuid_prefix(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    let Some((last, whole)) = groups.split_last() else {
        return false;
    };
    let hex = |g: &str| g.bytes().all(|b| b.is_ascii_hexdigit());
    id.len() >= crate::session::MIN_ID_PREFIX
        && groups.len() <= UUID_GROUPS.len()
        && whole
            .iter()
            .zip(UUID_GROUPS)
            .all(|(g, len)| g.len() == len && hex(g))
        && !last.is_empty()
        && last.len() <= UUID_GROUPS[whole.len()]
        && hex(last)
}

/// The hashes a transcript turn's text can match a typed prompt by: of the
/// whole text and of each text block, as the `Stop` hook matches cites.
#[must_use]
pub fn prompt_hashes(message: &crate::transcript::Message) -> Vec<String> {
    let mut hashes = vec![prompt_sha256(&message.text())];
    for block in &message.blocks {
        let hash = prompt_sha256(block);
        if !hashes.contains(&hash) {
            hashes.push(hash);
        }
    }
    hashes
}

/// SHA-256 of a prompt's text, lowercase hex: the cite's `prompt_sha256`,
/// which the `Stop` hook matches against the transcript, and a trigger
/// fire's key in the audit log (2_design.md §11.4).
#[must_use]
pub fn prompt_sha256(prompt: &str) -> String {
    bytes_sha256(prompt.as_bytes())
}

/// SHA-256 of raw bytes, lowercase hex, for checks on a transcript boundary.
#[must_use]
pub fn bytes_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Where and when a prompt was typed: everything an entry records about its
/// turn besides what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    /// The Claude Code session.
    pub session: SessionId,
    /// When the hook received the prompt, or the transcript timestamp for
    /// a prompt recovered after its hook was missed.
    pub at: OffsetDateTime,
    /// [`prompt_sha256`] of the whole prompt.
    pub prompt_sha256: String,
    /// The session's working directory.
    pub cwd: String,
    /// The repository the cwd belongs to.
    pub project: Project,
    /// The tool it was typed in, such as `claude-code`.
    pub provider: Label,
    /// The host it was typed on.
    pub host: Label,
}

impl Turn {
    /// The cite of `#scribe` line `line` of this turn; its message uuid is
    /// `None` until the next `Stop` hook finds it.
    #[must_use]
    pub fn cite(&self, line: u32) -> Cite {
        Cite {
            session: self.session.clone(),
            message: None,
            at: self.at,
            prompt_sha256: self.prompt_sha256.clone(),
            line,
            cwd: self.cwd.clone(),
        }
    }
}

/// The window an action entry keeps (§4.6): the transcript's turns since
/// the last user message before the prompt that carried a `#scribe` line,
/// oldest first, then the prompt itself (`message: None`, at `at`). The
/// prompt and everything after it are left out of the transcript part when
/// a retried hook finds it there, as [`window`] does. Each turn is cut to
/// [`WINDOW_TURN_CHARS`]; then the oldest turns are dropped until at most
/// [`WINDOW_TURNS`] turns and [`WINDOW_CHARS`] characters remain, and the
/// window records that it was truncated and how many turns it dropped. The
/// prompt itself is always kept. Model-free.
#[must_use]
pub fn action_window(
    messages: &[Message],
    prompt_sha256: &str,
    prompt: &str,
    at: OffsetDateTime,
) -> Window {
    let end = prompt_index(messages, prompt_sha256).unwrap_or(messages.len());
    let before = &messages[..end];
    let start = before
        .iter()
        .rposition(|m| {
            m.role == Speaker::User
                && (!lines(&m.text()).is_empty() || !crate::annotation::marks(&m.text()).is_empty())
        })
        .map_or(0, |i| i + 1);
    let mut turns: Vec<WindowTurn> = before[start..]
        .iter()
        .filter(|m| !m.text().trim().is_empty())
        .map(|m| window_turn(m.role, Some(m.uuid.clone()), m.at, &m.text()))
        .collect();
    turns.push(window_turn(Speaker::User, None, at, prompt));
    let mut kept: Vec<WindowTurn> = Vec::new();
    let mut chars = 0usize;
    for turn in turns.iter().rev() {
        let len = turn.text.chars().count();
        if !kept.is_empty() && (kept.len() >= WINDOW_TURNS || chars + len > WINDOW_CHARS) {
            break;
        }
        chars += len;
        kept.push(turn.clone());
    }
    kept.reverse();
    let omitted = turns.len() - kept.len();
    Window {
        turns: kept,
        truncated: omitted > 0,
        omitted: u32::try_from(omitted).unwrap_or(u32::MAX),
    }
}

fn window_turn(
    role: Speaker,
    message: Option<String>,
    at: OffsetDateTime,
    text: &str,
) -> WindowTurn {
    let text = text.trim();
    let cut = text.chars().count() > WINDOW_TURN_CHARS;
    WindowTurn {
        role,
        message,
        at,
        text: if cut {
            text.chars().take(WINDOW_TURN_CHARS).collect()
        } else {
            text.to_owned()
        },
        cut,
    }
}

/// The index of the user message that carries the prompt hashed to
/// `prompt_sha256`, the last one when there are several.
fn prompt_index(messages: &[Message], prompt_sha256: &str) -> Option<usize> {
    messages.iter().rposition(|m| {
        m.role == Speaker::User
            && (self::prompt_sha256(&m.text()) == prompt_sha256
                || m.blocks
                    .iter()
                    .any(|b| self::prompt_sha256(b) == prompt_sha256))
    })
}
