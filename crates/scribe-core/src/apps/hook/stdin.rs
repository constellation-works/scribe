//! A hook's stdin as its command: the payload decoded
//! (`scribe_tools::payload`) and the event's input built from it, inside the
//! audit guard, so a payload that cannot be read or decoded is still one
//! failure line with its code.

use super::{
    CodexInput, CodexStop, Command, Outcome, PromptInput, ReadPast, SessionStartInput, StopInput,
    refused, run,
};
use crate::SessionId;
use crate::composition::Context;
use crate::runtime::audit::{AuditMeta, AuditSubject};
use crate::runtime::errors::CommandError;
use scribe_tools::payload::{
    self, PromptPayload, SessionEndPayload, SessionStartPayload, StopPayload,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// A hook's stdin as the binary read it: the bytes, or why they could not be
/// read (an I/O error, or a payload over the size limit).
pub type Stdin = Result<Vec<u8>, CommandError>;

/// One hook run: the event and its stdin.
pub enum Event<'i> {
    /// `hook session-start`.
    SessionStart(Stdin),
    /// `hook user-prompt-submit`, with the reader `/scribe group add` uses
    /// for a named session's past transcript.
    UserPromptSubmit {
        /// The stdin.
        input: Stdin,
        /// Reads a named session's past transcript.
        read_past: ReadPast<'i, CommandError>,
    },
    /// `hook stop`.
    Stop(Stdin),
    /// `hook session-end`.
    SessionEnd(Stdin),
    /// Any of the four under Codex: `event` is the hook event the payload
    /// must name.
    Codex {
        /// The hook event.
        event: HookEvent,
        /// The stdin.
        input: Stdin,
        /// Reads a named session's past transcript.
        read_past: ReadPast<'i, CommandError>,
    },
}

/// One of the four provider lifecycle hook events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    /// `SessionStart`.
    SessionStart,
    /// `UserPromptSubmit`.
    UserPromptSubmit,
    /// `Stop`.
    Stop,
    /// `SessionEnd`.
    SessionEnd,
}

impl HookEvent {
    /// The event's name, as a payload's `hook_event_name` spells it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Stop => "Stop",
            Self::SessionEnd => "SessionEnd",
        }
    }

    /// The hook verb's command path in the audit log.
    const fn command(self) -> &'static str {
        match self {
            Self::SessionStart => "hook session-start",
            Self::UserPromptSubmit => "hook user-prompt-submit",
            Self::Stop => "hook stop",
            Self::SessionEnd => "hook session-end",
        }
    }
}

impl Event<'_> {
    /// What the audit log records for this hook: every hook is audited,
    /// under its command path, whichever provider runs it. Each reports its
    /// writes and refusals; a silent read has no audit line. Every hook calls
    /// no model and commits as one store transaction with its audit lines.
    /// Exhaustive, so a new event cannot compile without deciding (STD-02 R27).
    #[must_use]
    pub fn audit(&self) -> Option<AuditMeta> {
        let event = match self {
            Self::SessionStart(_) => HookEvent::SessionStart,
            Self::UserPromptSubmit { .. } => HookEvent::UserPromptSubmit,
            Self::Stop(_) => HookEvent::Stop,
            Self::SessionEnd(_) => HookEvent::SessionEnd,
            Self::Codex { event, .. } => *event,
        };
        Some(AuditMeta {
            command: event.command(),
            stepwise: false,
        })
    }

    /// The record and agent a hook names before it runs: none, since a
    /// hook names its session in each audit line.
    #[must_use]
    pub fn subject(&self, _ctx: &Context) -> AuditSubject {
        AuditSubject::default()
    }
}

/// The payload in `input`.
fn decode<T: DeserializeOwned>(input: &[u8]) -> Result<T, CommandError> {
    payload::decode(input).map_err(CommandError::HookPayload)
}

/// The payload's session id, validated.
fn session(id: &str) -> Result<SessionId, CommandError> {
    payload::session(id).map_err(CommandError::HookPayload)
}

/// The session a `SessionEnd` payload names.
fn session_end(input: &[u8]) -> Result<SessionId, CommandError> {
    let payload: SessionEndPayload = decode(input)?;
    session(&payload.session_id)
}

/// Run `event` against `ctx`.
pub(super) fn run_event(ctx: &Context, event: Event<'_>) -> Result<Outcome, CommandError> {
    match event {
        Event::SessionStart(input) => session_start(ctx, &input?),
        Event::UserPromptSubmit { input, read_past } => {
            let input = input?;
            let payload: PromptPayload = decode(&input)?;
            let session = session(&payload.session_id)?;
            let command = Command::UserPromptSubmit {
                input: PromptInput {
                    session: &session,
                    transcript_path: &payload.transcript_path,
                    cwd: &payload.cwd,
                    prompt: &payload.prompt,
                    agent_originated: payload::agent_originated(&payload),
                    provider: ctx.provider(),
                    host: ctx.host(),
                },
                read_past,
            };
            Ok(run(&ctx.core(), command)?)
        }
        Event::Stop(input) => {
            let input = input?;
            let payload: StopPayload = decode(&input)?;
            let session = session(&payload.session_id)?;
            let command = Command::Stop(StopInput {
                session: &session,
                transcript_path: &payload.transcript_path,
                cwd: payload.cwd,
                provider: ctx.provider(),
                host: ctx.host(),
            });
            Ok(run(&ctx.core(), command)?)
        }
        Event::SessionEnd(input) => {
            let command = Command::SessionEnd(session_end(&input?)?);
            Ok(run(&ctx.core(), command)?)
        }
        Event::Codex {
            event,
            input,
            read_past,
        } => codex(ctx, event.name(), &input?, read_past),
    }
}

/// `SessionStart` on `input`.
fn session_start(ctx: &Context, input: &[u8]) -> Result<Outcome, CommandError> {
    let payload: SessionStartPayload = decode(input)?;
    let session = session(&payload.session_id)?;
    let command = Command::SessionStart(SessionStartInput {
        session: &session,
        transcript_path: payload.transcript_path.as_deref(),
        source: payload.source.as_deref(),
        cwd: payload.cwd.as_deref(),
        interactive: ctx.interactive(),
        provider: ctx.provider(),
    });
    Ok(run(&ctx.core(), command)?)
}

/// A Codex hook: the payload, the event check and `SessionStart` here, then
/// the hook app's `Codex` command.
fn codex(
    ctx: &Context,
    event: &'static str,
    input: &[u8],
    read_past: ReadPast<'_, CommandError>,
) -> Result<Outcome, CommandError> {
    let value: Value = decode(input)?;
    if value["hook_event_name"] != event {
        return Ok(Outcome::Replied(Box::new(refused(
            event,
            "Codex hook_event_name is missing or does not match this entry point",
        ))));
    }
    if event == "SessionStart" {
        return session_start(ctx, input);
    }
    // Decoded here, raised by the case where the old adapter decoded them.
    let session_end = (event == "SessionEnd").then(|| session_end(input));
    let stop = (event == "Stop").then(|| {
        let payload: StopPayload = decode(input)?;
        Ok(CodexStop {
            session: session(&payload.session_id)?,
            transcript_path: payload.transcript_path,
            cwd: payload.cwd,
        })
    });
    let command = Command::Codex(CodexInput {
        event,
        payload: &value,
        provider: ctx.provider(),
        host: ctx.host(),
        session_end,
        stop,
        read_past,
    });
    Ok(run(&ctx.core(), command)?)
}
