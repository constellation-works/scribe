//! The instruction check: whether a write an agent runs on Daniel's word is
//! backed by a turn he typed in an interactive session, recently, naming the
//! target and the act (docs/design/scribe/2_design.md §12.2, §12.5).
//!
//! The CLI finds the session's transcript and reads it through the
//! provenance check (§4.1), before it takes the store lock; this module gets
//! the bytes and decides, with model-free word lists and no I/O (STD-02 R5).
//! [`claude_conversation`] and [`codex_conversation`] turn a transcript into
//! [`Spoken`] messages, marking the turns Daniel plainly typed and whether
//! the harness recorded an interactive entry point for them; [`check`] runs
//! checks 2 to 5 of §12.2 and the interactive check of §12.5 over them.
//!
//! It is an accident guard (STD-05 R5), like every provenance check in
//! scribe: it stops an agent acting on its own, a stale or misread
//! instruction and an instruction meant for another record. A process
//! running as Daniel can write a transcript file, and the word lists do not
//! understand meaning.

use crate::capture;
use crate::entry::Speaker;
use crate::error::{Error, WriteRefusal};
use crate::session::SessionId;
use crate::transcript::{self, Message};
use crate::verbatim;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use time::OffsetDateTime;

/// How many of the session's most recent typed turns an instruction may be
/// in (§12.2 check 3).
pub const RECENT_TURNS: usize = 3;

/// How old the instruction turn may be when the command runs (§12.2 check 3).
pub const FRESH_FOR: time::Duration = time::Duration::minutes(30);

/// Claude Code `entrypoint` values of a session Daniel types into: the
/// terminal (`cli`) and the desktop app (`claude-desktop`). `sdk-cli` is
/// `claude -p` and the Agent SDK, which every Orbit run, routine and drain
/// uses; it and every other value are refused (§12.5).
pub const INTERACTIVE_ENTRYPOINTS: &[&str] = &["cli", "claude-desktop"];

/// Codex `session_meta.payload.source` values of a session Daniel types
/// into: the terminal UI (`cli`) and the IDE or desktop app (`vscode`).
/// `exec` (`codex exec`, the non-interactive route) and a subagent's source
/// are refused (§12.5).
pub const CODEX_INTERACTIVE_SOURCES: &[&str] = &["cli", "vscode"];

/// The act a write performs on Daniel's word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// `proposal settle --approve`.
    Approve,
    /// `proposal settle --reject`.
    Reject,
    /// `entry retire`.
    Retire,
    /// `entry forget`.
    Forget,
}

impl Act {
    /// The verb, as the refusal names it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::Retire => "retire",
            Self::Forget => "forget",
        }
    }

    /// The words or phrases in his own turn that name this act (§12.2
    /// check 5). For [`Act::Approve`] these are the explicit ones; the bare
    /// affirmatives are [`AFFIRMATIVES`].
    #[must_use]
    pub fn words(self) -> &'static [&'static str] {
        match self {
            Self::Approve => &["approve", "accept", "keep"],
            Self::Reject => &["reject", "decline", "drop", "discard", "delete", "remove"],
            Self::Retire => &["retire", "delete", "remove", "hide", "drop"],
            Self::Forget => &["forget", "erase", "purge", "permanently", "for good"],
        }
    }
}

impl fmt::Display for Act {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Bare affirmatives: they approve only when the reply he answered asks to
/// approve or accept the target, and they never reject, retire or forget.
pub const AFFIRMATIVES: &[&str] = &["yes", "ok", "okay", "go ahead", "do it"];

/// Words of the answered reply that make a bare affirmative an approval,
/// when they sit on the same line as the target's id.
pub const APPROVE_IN_REPLY: &[&str] = &["approve", "accept"];

/// One message of the session, as the check reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spoken {
    /// The message as the conversation holds it.
    pub message: Message,
    /// For a user turn Daniel plainly typed (§4.1's origin guard), his own
    /// words: pasted blocks, code fences and blockquotes left out. `None`
    /// for an assistant message and for a user turn that is not his.
    pub typed: Option<String>,
    /// The harness entry point the transcript records for this message, as
    /// the refusal names it.
    pub entrypoint: Option<String>,
    /// Whether that entry point is an interactive session's.
    pub interactive: bool,
}

/// What the command acts on: its id and, when the command was given a
/// unique prefix, the prefix it resolved. Either spelling in his turn or in
/// the reply he answered names the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The full id.
    pub id: String,
    /// The text the command was given, when it differs from `id`.
    pub typed: Option<String>,
}

impl Target {
    /// The target `id`, given on the command line as `typed`.
    #[must_use]
    pub fn new(id: impl Into<String>, typed: &str) -> Self {
        let id = id.into();
        let typed = (typed != id).then(|| typed.to_owned());
        Self { id, typed }
    }

    fn spellings(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.id.as_str()).chain(self.typed.as_deref())
    }
}

/// What the command asks the check to back.
#[derive(Clone, Debug)]
pub struct Request<'a> {
    /// The session the instruction was typed in (`--session`).
    pub session: &'a SessionId,
    /// The verbatim span of his turn (`--instruction`).
    pub quote: &'a str,
    /// The act the command performs.
    pub act: Act,
    /// What it acts on.
    pub target: &'a Target,
    /// When the command runs.
    pub now: OffsetDateTime,
}

/// A checked instruction: the turn that backs the write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instructed {
    /// The session it was typed in.
    pub session: SessionId,
    /// The turn's message uuid.
    pub message: String,
    /// When he typed it.
    pub at: OffsetDateTime,
    /// The span the command quoted, as given.
    pub quote: String,
    /// SHA-256 of the turn's whole text, lowercase hex (a cite's
    /// `prompt_sha256`).
    pub prompt_sha256: String,
    /// The line of the turn the quote starts on, from 1.
    pub line: u32,
}

impl Instructed {
    /// The instruction turn as a cite: a settlement's `settled_by`
    /// (2_design.md §12.3). The transcript's working directory is not read,
    /// so `cwd` is empty.
    #[must_use]
    pub fn cite(&self) -> crate::Cite {
        crate::Cite {
            session: self.session.clone(),
            message: Some(self.message.clone()),
            at: self.at,
            prompt_sha256: self.prompt_sha256.clone(),
            line: self.line,
            cwd: String::new(),
        }
    }

    /// The record the store keeps of it, its quote masked by `mask`.
    #[must_use]
    pub fn record(&self, mask: impl Fn(&str) -> String) -> crate::Instruction {
        crate::Instruction {
            session: self.session.clone(),
            message: self.message.clone(),
            at: self.at,
            quote: Some(mask(&self.quote)),
        }
    }
}

/// The line of `text`, from 1, that the whole of `quote` starts on: the
/// last line from which the rest of `text` still holds it as a verbatim
/// span (§6.4), so a quote whose first line also appears earlier is placed
/// where it matched. 1 when no line does (a short answer that is his whole
/// turn).
fn quote_line(text: &str, quote: &str) -> u32 {
    let starts = std::iter::once(0).chain(text.match_indices('\n').map(|(at, _)| at + 1));
    let line = starts
        .enumerate()
        .filter(|&(_, at)| verbatim::contains(&text[at..], quote))
        .last()
        .map_or(0, |(line, _)| line);
    u32::try_from(line)
        .unwrap_or(u32::MAX - 1)
        .saturating_add(1)
}

/// The latest turn Daniel plainly typed that holds `quote` (§12.2 check 2),
/// as its rank among his typed turns of `spoken`.
fn locate(spoken: &[Spoken], quote: &str) -> Option<usize> {
    spoken
        .iter()
        .filter_map(|s| s.typed.as_deref())
        .collect::<Vec<_>>()
        .iter()
        .rposition(|text| quoted_in(text, quote))
}

/// The message id of the turn [`check`] would take as the instruction for
/// `quote`, when it finds one with a nonblank id, whether or not the turn
/// then passes the other checks. A refusal's audit line records it, so a
/// stale or mismatched instruction is still traceable to the turn; `None`
/// only when no typed turn holds the quote.
#[must_use]
pub fn located_message<'a>(spoken: &'a [Spoken], quote: &str) -> Option<&'a str> {
    let typed: Vec<&Spoken> = spoken.iter().filter(|s| s.typed.is_some()).collect();
    let rank = locate(spoken, quote)?;
    let uuid = typed.get(rank)?.message.uuid.as_str();
    (!uuid.trim().is_empty()).then_some(uuid)
}

/// Run §12.2 checks 2 to 5 and §12.5's interactive check over `spoken`, the
/// session's messages in order. The refusals, in the order checked:
///
/// - `instruction_not_found`: `quote` is in no turn Daniel plainly typed;
/// - `not_interactive`: the turn's entry point is not an interactive one;
/// - `instruction_stale`: the turn is not one of his last [`RECENT_TURNS`]
///   typed turns, or was typed more than [`FRESH_FOR`] before `now`;
/// - `instruction_mismatch`: neither the turn nor the assistant message just
///   before it names the target, either holds a negation or exception
///   marker anywhere (`has_exception`, conservative by design), or his turn
///   does not name the act.
///
/// # Errors
///
/// [`Error::AgentWrite`] with the refusal's [`WriteRefusal`].
pub fn check(spoken: &[Spoken], request: &Request<'_>) -> Result<Instructed, Error> {
    let typed: Vec<(usize, &str)> = spoken
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.typed.as_deref().map(|t| (i, t)))
        .collect();
    let Some(rank) = locate(spoken, request.quote) else {
        return Err(refuse(
            WriteRefusal::InstructionNotFound,
            format!(
                "--instruction is not word for word in a turn the user typed in session {}; quote their own words exactly (at least {} characters, or their whole turn)",
                request.session,
                verbatim::MIN_QUOTE_CHARS
            ),
        ));
    };
    let (index, words) = typed[rank];
    let turn = &spoken[index];
    if turn.message.uuid.trim().is_empty() {
        // Every record of the instruction names its turn; one without an
        // id cannot be cited, and a store holding it would not read.
        return Err(refuse(
            WriteRefusal::InstructionNotFound,
            format!(
                "the turn holding --instruction in session {} has no message id; ask the user again",
                request.session
            ),
        ));
    }
    if !turn.interactive {
        return Err(refuse(
            WriteRefusal::NotInteractive,
            format!(
                "session {} is not an interactive session (entry point {}); act on the user's instruction only in a session they type into",
                request.session,
                turn.entrypoint.as_deref().unwrap_or("none recorded")
            ),
        ));
    }
    let recent = typed.len() - rank <= RECENT_TURNS;
    let age = request.now - turn.message.at;
    if !recent || age < time::Duration::ZERO || age > FRESH_FOR {
        return Err(refuse(
            WriteRefusal::InstructionStale,
            format!(
                "the instruction is not one of the user's last {RECENT_TURNS} typed turns of the last {} minutes; ask them again",
                FRESH_FOR.whole_minutes()
            ),
        ));
    }
    let reply = index
        .checked_sub(1)
        .and_then(|i| spoken.get(i))
        .filter(|s| s.message.role == Speaker::Assistant)
        .map(|s| s.message.text());
    let reply = reply.as_deref();
    let named = names_target(words, request.target)
        || reply.is_some_and(|r| names_target(r, request.target));
    if !named {
        return Err(refuse(
            WriteRefusal::InstructionMismatch,
            format!(
                "neither the instruction nor the reply it answers names {}; name the id in your reply before acting",
                request.target.id
            ),
        ));
    }
    if has_exception(words) || reply.is_some_and(|r| has_exception(&without_codes(r))) {
        return Err(refuse(
            WriteRefusal::InstructionMismatch,
            String::from(
                "the instruction or reply contains a negation or exception; ask the user for a plainer instruction",
            ),
        ));
    }
    if !names_act(words, reply, request.act, request.target) {
        return Err(refuse(
            WriteRefusal::InstructionMismatch,
            format!(
                "the instruction does not say to {} {}; act only on what the user's words ask for",
                request.act, request.target.id
            ),
        ));
    }
    let text = turn.message.text();
    Ok(Instructed {
        session: request.session.clone(),
        message: turn.message.uuid.clone(),
        at: turn.message.at,
        quote: request.quote.to_owned(),
        prompt_sha256: capture::prompt_sha256(&text),
        line: quote_line(&text, request.quote),
    })
}

fn refuse(kind: WriteRefusal, message: String) -> Error {
    Error::AgentWrite { kind, message }
}

/// Whether `quote` is a verbatim span of `typed` (§6.4), or, for a short
/// answer such as "yes", the whole of it.
fn quoted_in(typed: &str, quote: &str) -> bool {
    if verbatim::contains(typed, quote) {
        return true;
    }
    let whole = verbatim::normalize(typed);
    let quote = verbatim::normalize(quote);
    let quote = quote.trim_matches(' ');
    !quote.is_empty() && whole.trim_matches(' ') == quote
}

/// Whether `text` names the target: one of its spellings, standing alone
/// (not part of a longer id, so `p-4` is not named by `p-45`).
#[must_use]
pub fn names_target(text: &str, target: &Target) -> bool {
    target.spellings().any(|id| mentions(text, id))
}

fn mentions(text: &str, id: &str) -> bool {
    let id_char = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_');
    !id.is_empty()
        && text.match_indices(id).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + id.len()..].chars().next();
            !before.is_some_and(id_char) && !after.is_some_and(id_char)
        })
}

/// §12.2 check 5: whether his own words name `act`. For [`Act::Approve`] a
/// bare affirmative counts only when `reply`, the message he answered, has
/// "approve" or "accept" on a line naming the target.
#[must_use]
pub fn names_act(words: &str, reply: Option<&str>, act: Act, target: &Target) -> bool {
    let tokens = tokens_of(words);
    if act.words().iter().any(|w| has_phrase(&tokens, w)) {
        return true;
    }
    if act != Act::Approve || !AFFIRMATIVES.iter().any(|w| has_phrase(&tokens, w)) {
        return false;
    }
    reply.is_some_and(|reply| {
        reply.lines().any(|line| {
            names_target(line, target) && {
                let line = tokens_of(line);
                APPROVE_IN_REPLY.iter().any(|w| has_phrase(&line, w))
            }
        })
    })
}

fn has_exception(text: &str) -> bool {
    let tokens = crate::claim::statement_tokens(text);
    tokens.iter().any(|token| {
        crate::claim::is_marker(token) || matches!(token.as_str(), "but" | "stop" | "cancel")
    }) || tokens
        .windows(2)
        .any(|pair| pair[0] == "other" && pair[1] == "than")
}

/// The reply without its snake_case words. A reply that echoes a refusal
/// code, such as `instruction_not_found`, would otherwise hold the marker
/// `not` and refuse the human's corrected retry that answers it. A code is
/// never the human's exception, so it is left out of the scan.
fn without_codes(reply: &str) -> String {
    reply
        .split_whitespace()
        .filter(|word| !word.contains('_'))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lowercased runs of letters and digits.
fn tokens_of(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether `phrase`'s words occur consecutively in `tokens`.
fn has_phrase(tokens: &[String], phrase: &str) -> bool {
    let words: Vec<&str> = phrase.split(' ').collect();
    !words.is_empty()
        && tokens
            .windows(words.len())
            .any(|w| w.iter().zip(&words).all(|(t, p)| t == p))
}

/// His own words in a typed turn: pasted blocks, code fences and
/// blockquotes left out, lines joined by `\n`.
#[must_use]
pub fn own_words(text: &str) -> String {
    capture::plain_lines(text)
        .into_iter()
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The per-line fields of a Claude Code transcript the check reads beside
/// the conversation: the entry point and the origin claims.
#[derive(Deserialize)]
struct LineMeta {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    uuid: Option<serde_json::Value>,
    #[serde(default)]
    entrypoint: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "capture::present_value")]
    source: Option<serde_json::Value>,
    #[serde(
        default,
        rename = "promptSource",
        deserialize_with = "capture::present_value"
    )]
    prompt_source_camel: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "capture::present_value")]
    prompt_source: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "capture::present_value")]
    origin: Option<serde_json::Value>,
    #[serde(
        default,
        rename = "promptOrigin",
        deserialize_with = "capture::present_value"
    )]
    prompt_origin_camel: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "capture::present_value")]
    prompt_origin: Option<serde_json::Value>,
    #[serde(
        default,
        rename = "turnOrigin",
        deserialize_with = "capture::present_value"
    )]
    turn_origin_camel: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "capture::present_value")]
    turn_origin: Option<serde_json::Value>,
    #[serde(default)]
    attachment: Option<LineAttachment>,
}

/// The attachment of a queued message, whose origin is the claim it carries.
#[derive(Deserialize)]
struct LineAttachment {
    #[serde(default)]
    origin: Option<serde_json::Value>,
}

struct Claims {
    origin_blocked: bool,
    entrypoint: Option<String>,
}

fn string(value: Option<&serde_json::Value>) -> Option<String> {
    value.and_then(serde_json::Value::as_str).map(str::to_owned)
}

/// Read the conversation with copied histories from other sessions excluded.
#[must_use]
pub fn claude_session_conversation(bytes: &[u8], session: &SessionId) -> Vec<Spoken> {
    claude_conversation(&claude_session_lines(bytes, session))
}

/// Verified Claude turns with the same text selection as the session pass.
#[must_use]
pub fn claude_group_turns(bytes: &[u8], session: &SessionId) -> Vec<crate::message::NewMessage> {
    read_claude_conversation(&claude_session_lines(bytes, session), str::to_owned)
        .into_iter()
        .filter(|turn| turn.entrypoint.is_none() || turn.interactive)
        .filter_map(|turn| {
            crate::message::typed_turn(session, &turn.message, turn.typed.as_deref()?)
        })
        .collect()
}

fn claude_session_lines(bytes: &[u8], session: &SessionId) -> Vec<u8> {
    let mut local = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        let Ok(row) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if row
            .get("sessionId")
            .is_some_and(|id| id.as_str() != Some(session.as_str()))
        {
            continue;
        }
        local.extend_from_slice(line);
        local.push(b'\n');
    }
    local
}

/// A Claude Code transcript as the check reads it: the conversation
/// ([`transcript::parse`]), each user turn marked as Daniel's when
/// [`transcript::user_turn`] keeps it and §4.1's origin guard
/// ([`capture::OriginClaims`]) passes its line's origin fields and
/// text, and each message's `entrypoint` checked against
/// [`INTERACTIVE_ENTRYPOINTS`]. A replayed line keeps its first copy's
/// fields, as the conversation keeps its first copy.
#[must_use]
pub fn claude_conversation(bytes: &[u8]) -> Vec<Spoken> {
    read_claude_conversation(bytes, own_words)
}

/// Read origin-verified typed turns without removing fenced or quoted text.
#[must_use]
pub fn claude_conversation_raw_typed(bytes: &[u8]) -> Vec<Spoken> {
    read_claude_conversation(bytes, str::to_owned)
}

fn read_claude_conversation(bytes: &[u8], own: impl Fn(&str) -> String) -> Vec<Spoken> {
    let claims = claude_claims(bytes);
    transcript::parse(bytes)
        .into_iter()
        .map(|message| {
            let claim = claims.get(&message.uuid);
            let entrypoint = claim.and_then(|c| c.entrypoint.clone());
            let typed = verified_words(&message, claim.is_some_and(|c| !c.origin_blocked))
                .map(|words| own(&words))
                .filter(|own| !own.trim().is_empty());
            let interactive = entrypoint
                .as_deref()
                .is_some_and(|e| INTERACTIVE_ENTRYPOINTS.contains(&e));
            Spoken {
                message,
                typed,
                entrypoint,
                interactive,
            }
        })
        .collect()
}

fn verified_words(message: &Message, permitted: bool) -> Option<String> {
    let turn = transcript::user_turn(message)?;
    let text = message.text();
    // The guard reads his words: a slash command's args, not its harness tags.
    let checked = transcript::slash_command_args(&text).unwrap_or(&text);
    (permitted && !capture::agent_originated(&[], checked)).then_some(turn.own)
}

fn claude_claims(bytes: &[u8]) -> BTreeMap<String, Claims> {
    let mut claims: BTreeMap<String, Claims> = BTreeMap::new();
    for line in bytes.split(|&b| b == b'\n') {
        let Ok(meta) = serde_json::from_slice::<LineMeta>(line) else {
            continue;
        };
        let Some(uuid) = string(meta.uuid.as_ref()) else {
            continue;
        };
        match meta.kind.as_deref() {
            Some("user" | "assistant") => {
                claims.entry(uuid).or_insert_with(|| {
                    let mut origin = capture::OriginClaims::default();
                    origin.push_claim(meta.source.as_ref());
                    origin.push_prompt_source(meta.prompt_source_camel.as_ref());
                    origin.push_prompt_source(meta.prompt_source.as_ref());
                    origin.push_origin(meta.origin.as_ref());
                    origin.push_claim(meta.prompt_origin_camel.as_ref());
                    origin.push_claim(meta.prompt_origin.as_ref());
                    origin.push_turn_origin(meta.turn_origin_camel.as_ref());
                    origin.push_turn_origin(meta.turn_origin.as_ref());
                    Claims {
                        origin_blocked: origin.claims_agent_originated(),
                        entrypoint: string(meta.entrypoint.as_ref()),
                    }
                });
            }
            // Only a queued message is a conversation line here: the same
            // predicate that makes it a message gates its claims.
            Some("attachment") if transcript::queued_message(line).is_some() => {
                claims.entry(uuid).or_insert_with(|| {
                    let mut origin = capture::OriginClaims::default();
                    origin.push_origin(meta.attachment.as_ref().and_then(|a| a.origin.as_ref()));
                    Claims {
                        origin_blocked: origin.claims_agent_originated(),
                        entrypoint: string(meta.entrypoint.as_ref()),
                    }
                });
            }
            _ => {}
        }
    }
    claims
}

/// Parsed Claude transcript lines for group capture. A batch can be the
/// whole transcript or a suffix starting at a JSONL record boundary.
pub struct ClaudeGroupTurns {
    session: SessionId,
    messages: Vec<Message>,
    claims: BTreeMap<String, bool>,
}

/// The first origin claim and whether a parseable message has already been
/// seen for an id. These are separate because a metadata line can supply
/// claims even when it does not produce a message.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ClaudeSeen {
    pub(crate) permitted: Option<bool>,
    pub(crate) message: bool,
}

impl ClaudeGroupTurns {
    /// Apply the session filter and parse this batch before taking a store lock.
    #[must_use]
    pub fn parse(bytes: &[u8], session: &SessionId) -> Self {
        let local = claude_session_lines(bytes, session);
        Self {
            session: session.clone(),
            messages: transcript::parse_claude(&local),
            claims: claude_claims(&local)
                .into_iter()
                .map(|(uuid, claim)| {
                    let interactive = claim
                        .entrypoint
                        .as_deref()
                        .is_none_or(|e| INTERACTIVE_ENTRYPOINTS.contains(&e));
                    (uuid, interactive && !claim.origin_blocked)
                })
                .collect(),
        }
    }

    /// Whether there are messages for which the caller may need its redactor.
    #[must_use]
    pub fn has_user_messages(&self) -> bool {
        self.messages.iter().any(|m| m.role == Speaker::User)
    }

    pub(crate) fn session(&self) -> &SessionId {
        &self.session
    }

    /// Keep first-copy semantics across batches, including ids whose first
    /// copy was rejected. The index only receives hashed ids and flags.
    pub(crate) fn resolve<E>(
        mut self,
        mut lookup: impl FnMut(&str) -> Result<ClaudeSeen, E>,
    ) -> Result<ResolvedClaudeTurns, E> {
        let mut turns = Vec::new();
        let mut seen = Vec::new();
        for message in self.messages {
            let id = capture::prompt_sha256(&message.uuid);
            let previous = lookup(&id)?;
            let permitted = previous.permitted.or(self.claims.remove(&message.uuid));
            let current = ClaudeSeen {
                permitted,
                message: true,
            };
            if current != previous {
                seen.push((id, current));
            }
            if !previous.message
                && let Some(words) = verified_words(&message, permitted == Some(true))
                && let Some(turn) = crate::message::typed_turn(&self.session, &message, &words)
            {
                turns.push(turn);
            }
        }
        for (uuid, permitted) in self.claims {
            let id = capture::prompt_sha256(&uuid);
            let mut previous = lookup(&id)?;
            if previous.permitted.is_none() {
                previous.permitted = Some(permitted);
                seen.push((id, previous));
            }
        }
        Ok(ResolvedClaudeTurns { turns, seen })
    }
}

pub(crate) struct ResolvedClaudeTurns {
    pub(crate) turns: Vec<crate::message::NewMessage>,
    pub(crate) seen: Vec<(String, ClaudeSeen)>,
}

/// A Codex rollout as the check reads it. The session's interactivity is
/// its `session_meta.payload.source`: [`CODEX_INTERACTIVE_SOURCES`] pass,
/// and any other source (`exec`, a subagent's) is refused
/// `not_interactive` before the rollout is parsed. The typed turns are
/// those [`crate::codex::parse`] attributes to Daniel by the host's content
/// provenance.
///
/// # Errors
///
/// `not_interactive` for a non-interactive source, and
/// `instruction_not_found` when the rollout is not one this build reads or
/// belongs to another session.
pub fn codex_conversation(bytes: &[u8], session: &SessionId) -> Result<Vec<Spoken>, Error> {
    let source = codex_source(bytes);
    let entrypoint = format!("codex {}", source.as_deref().unwrap_or("none recorded"));
    if !source
        .as_deref()
        .is_some_and(|s| CODEX_INTERACTIVE_SOURCES.contains(&s))
    {
        return Err(refuse(
            WriteRefusal::NotInteractive,
            format!(
                "session {session} is not an interactive session (entry point {entrypoint}); act on the user's instruction only in a session they type into"
            ),
        ));
    }
    let rollout = crate::codex::parse(bytes).map_err(|why| {
        refuse(
            WriteRefusal::InstructionNotFound,
            format!("cannot read session {session}'s Codex transcript: {why}"),
        )
    })?;
    if rollout.session != *session {
        return Err(refuse(
            WriteRefusal::InstructionNotFound,
            format!("the Codex transcript found is not session {session}'s"),
        ));
    }
    Ok(rollout
        .messages
        .into_iter()
        .map(|message| {
            let typed = transcript::user_turn(&message)
                .map(|turn| own_words(&turn.own))
                .filter(|own| !own.trim().is_empty());
            Spoken {
                message,
                typed,
                entrypoint: Some(entrypoint.clone()),
                interactive: true,
            }
        })
        .collect())
}

/// The `source` of the rollout's first `session_meta` line, when it is a
/// string.
fn codex_source(bytes: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(rename = "type", default)]
        kind: Option<serde_json::Value>,
        #[serde(default)]
        payload: Option<Payload>,
    }
    #[derive(Deserialize)]
    struct Payload {
        #[serde(default)]
        source: Option<serde_json::Value>,
    }
    bytes
        .split(|&b| b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Row>(line).ok())
        .find(|row| row.kind.as_ref().and_then(serde_json::Value::as_str) == Some("session_meta"))
        .and_then(|row| string(row.payload.and_then(|p| p.source).as_ref()))
}
