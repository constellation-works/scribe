//! The `hook` app: the provider lifecycle hooks (`scribe hook …`;
//! 2_design.md §4, §5, §14) — SessionStart, UserPromptSubmit, Stop and
//! SessionEnd under Claude Code, and the Codex adapter — with the `/scribe`
//! controls, the `#scribe` settling lines and the Stop hand-over they run.
//!
//! A hook decodes nothing: the binary decodes the payload and builds the
//! [`Command`]. Every run replies with a [`HookReply`], whose `outcomes`
//! the audit guard writes one line each for.

mod annotation;
mod codex;
mod continuation;
mod group_add;
mod proposal_command;
mod reply;
pub mod rules;
mod scribe_command;
mod session_end;
mod session_start;
mod stdin;
mod stop;
#[cfg(test)]
mod tests;
mod user_prompt_submit;

pub use codex::{CodexInput, CodexStop, refused};
pub use group_add::{GroupAddInput, MAX_SESSIONS};
pub use reply::{HookPortError, HookReply, PROVENANCE_FAILED, chain, quote};
pub use scribe_tools::payload::PromptPayload;
pub use session_start::SessionStartInput;
pub use stdin::{Event, HookEvent, Stdin};
pub use stop::StopInput;
pub use user_prompt_submit::{EVENT, PromptInput};

use crate::apps::group::HookError;
use crate::composition::Context;
use crate::message::NewMessage;
use crate::runtime::Ctx;
use crate::runtime::audit::{Audit, Audited, Line, guarded};
use crate::runtime::errors::CommandError;
use crate::{Provider, SessionId};
use std::path::Path;

/// Reads a named session's past transcript for `/scribe group add`: its
/// typed turns, or `None` when it has no transcript.
pub type ReadPast<'i, E> =
    &'i mut dyn FnMut(Provider, &SessionId, &Path) -> Result<Option<Vec<NewMessage>>, E>;

/// A `hook` command: one lifecycle event, its payload decoded.
pub enum Command<'i, E> {
    /// Claude Code's `SessionStart`.
    SessionStart(SessionStartInput<'i>),
    /// Claude Code's `UserPromptSubmit`.
    UserPromptSubmit {
        /// The prompt and its session.
        input: PromptInput<'i>,
        /// Reads a past transcript for `/scribe group add`.
        read_past: ReadPast<'i, E>,
    },
    /// Claude Code's `Stop`.
    Stop(StopInput<'i>),
    /// `SessionEnd`: the session that ended.
    SessionEnd(SessionId),
    /// A Codex lifecycle event other than `SessionStart`. A verified
    /// `UserPromptSubmit` turn's `#scribe` markers are echoed in its reply.
    Codex(CodexInput<'i, E, ReadPast<'i, E>>),
}

/// What a `hook` command returns.
pub enum Outcome {
    /// The reply the hook prints, with its audit outcomes.
    Replied(Box<HookReply>),
}

/// Why a `hook` command failed. A refusal is a reply, not an error.
pub type Error<E> = HookError<E>;

/// One line per write or refusal the run reports, and none when it changed
/// nothing.
impl Audited for Outcome {
    fn lines(&self) -> Vec<Line> {
        let Self::Replied(reply) = self;
        reply
            .outcomes
            .iter()
            .map(|outcome| match outcome.error_code {
                Some(code) => Line::failure(outcome.target.clone(), code),
                None => Line::success(outcome.target.clone()),
            })
            .collect()
    }
}

/// Run `event` against `ctx`, inside the audit guard [`Event::audit`]
/// declares: its stdin decoded, then the event's command.
///
/// # Errors
///
/// [`CommandError`]: stdin that could not be read or decoded, a store error,
/// an unreadable transcript, or a port.
pub fn exec(ctx: &Context, event: Event<'_>) -> Result<Outcome, CommandError> {
    let audit = Audit::declared(event.audit(), || event.subject(ctx));
    guarded(ctx, audit, || stdin::run_event(ctx, event))
}

/// Run `event` against `ctx` with no audit line, as the binary's prompt
/// hook tests drive it.
///
/// # Errors
///
/// As [`exec`].
#[cfg(any(test, feature = "test-support"))]
pub fn exec_unaudited(ctx: &Context, event: Event<'_>) -> Result<Outcome, CommandError> {
    guarded(ctx, None, || stdin::run_event(ctx, event))
}

/// Run `command` against `ctx`: dispatch only.
pub(crate) fn run<E: HookPortError>(
    ctx: &Ctx<'_, E>,
    command: Command<'_, E>,
) -> Result<Outcome, Error<E>> {
    let reply = match command {
        Command::SessionStart(input) => session_start::run(ctx, &input),
        Command::UserPromptSubmit { input, read_past } => {
            user_prompt_submit::run(ctx, &input, read_past)?
        }
        Command::Stop(input) => stop::run(ctx, input)?,
        Command::SessionEnd(session) => session_end::run(ctx, &session)?,
        Command::Codex(input) => {
            let (event, prompt) = (
                input.event,
                input.payload["prompt"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
            let reply = codex::run(ctx, input)?;
            if event == EVENT {
                codex::with_mark_echoes(reply, &prompt)
            } else {
                reply
            }
        }
    };
    Ok(Outcome::Replied(Box::new(reply)))
}

/// Apply a typed `/scribe group add` line (the test entry point; the prompt
/// hook runs it in production).
///
/// # Errors
///
/// [`HookError`]: the config or the redactor failed. A refusal is a reply.
#[cfg(any(test, feature = "test-support"))]
pub fn group_add(
    ctx: &Context,
    input: &GroupAddInput<'_>,
    read_past: ReadPast<'_, CommandError>,
) -> Result<HookReply, CommandError> {
    Ok(group_add::run(&ctx.core(), input, read_past)?)
}
