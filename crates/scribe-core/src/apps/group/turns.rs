//! Capture a group member's typed turns from host-checked transcript data
//! (ORB-27707 step 3b): the Claude transcript's new records at a prompt or a
//! Stop, a Codex rollout's typed turns, and a past session's turns when it
//! joins.

use super::HookError;
use super::verbs::GroupVerbs;
use crate::apps::tag::ConfigReads;
use crate::control::is_control;
use crate::runtime::audit::HookOutcome;
use crate::runtime::{Ctx, PortError, Unreadable};
use crate::{Provider, SessionId, codex::TypedTurn, message, transcript};
use std::path::Path;

/// Capture `session`'s new Claude transcript records at `path`, when it is
/// an open group member; one outcome when turns were stored.
///
/// # Errors
///
/// [`HookError`]: a store error, an unreadable transcript, or the redactor.
#[cfg(unix)]
pub fn claude<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    session: &SessionId,
    path: &Path,
) -> Result<Option<HookOutcome>, HookError<E>> {
    use crate::runtime::ClaudeRead;
    use crate::{Redactor, instruction::ClaudeGroupTurns};

    // A concurrent prompt or Stop may commit between the read and write.
    // Retry from its newer snapshot without holding a lock over file I/O.
    for _ in 0..3 {
        let Some(before) = ctx.store.claude_group_capture(session)? else {
            return Ok(None);
        };
        let read = checked(
            ctx.transcripts
                .claude_window(provider, path, session, before.progress()),
            path,
            session,
        )?;
        let window = match read {
            None | Some(ClaudeRead::Window(None)) => return Ok(None),
            Some(ClaudeRead::Retry) => continue,
            Some(ClaudeRead::Window(Some(window))) => window,
        };
        let batch = ClaudeGroupTurns::parse(&window.bytes, session);
        let redactor = if batch.has_user_messages() {
            let config = ctx.store.read().tags().config()?.for_provider(provider);
            ctx.redaction.redactor(&config).map_err(HookError::Port)?
        } else {
            Redactor::default()
        };
        if let Some(inserted) = ctx.store.capture_claude_group_turns(
            &before,
            window.resumed,
            &window.position,
            batch,
            &redactor,
        )? {
            return Ok(captured(session, inserted));
        }
    }
    tracing::debug!(%session, "transcript or group capture progress changed during every attempt");
    Ok(None)
}

/// Capture `session`'s Claude transcript at `path`, when it is an open
/// group member; one outcome when turns were stored.
///
/// # Errors
///
/// [`HookError`]: a store error, an unreadable transcript, or the redactor.
#[cfg(not(unix))]
pub fn claude<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    session: &SessionId,
    path: &Path,
) -> Result<Option<HookOutcome>, HookError<E>> {
    if !ctx.store.captures_group_turns(session)? {
        return Ok(None);
    }
    let Some(bytes) = checked(
        ctx.transcripts.read_checked(provider, path, session),
        path,
        session,
    )?
    else {
        return Ok(None);
    };
    save(
        ctx,
        provider,
        session,
        &crate::instruction::claude_group_turns(&bytes, session),
    )
}

fn checked<T, E>(
    result: Result<Option<T>, Unreadable>,
    path: &Path,
    session: &SessionId,
) -> Result<Option<T>, HookError<E>> {
    match result {
        Ok(found) => Ok(found),
        Err(Unreadable::Refused(reason)) => {
            tracing::warn!(%session, %reason, "group turn capture refused the transcript");
            Ok(None)
        }
        Err(Unreadable::Io(source)) => Err(HookError::Read {
            what: path.display().to_string(),
            source,
        }),
    }
}

/// Capture `session`'s typed Codex `turns`, when it is an open group
/// member; one outcome when turns were stored.
///
/// # Errors
///
/// [`HookError`]: a store error or the redactor.
pub fn codex<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    session: &SessionId,
    turns: &[TypedTurn],
) -> Result<Option<HookOutcome>, HookError<E>> {
    if !ctx.store.captures_group_turns(session)? {
        return Ok(None);
    }
    save(ctx, provider, session, &codex_messages(session, turns))
}

fn codex_messages(session: &SessionId, turns: &[TypedTurn]) -> Vec<message::NewMessage> {
    turns
        .iter()
        .filter_map(|turn| {
            let input = &turn.message;
            if is_control(&input.text()) {
                return None;
            }
            message::typed_turn(session, input, &transcript::user_turn(input)?.own)
        })
        .collect()
}

/// The typed turns of a past session whose transcript is at `path`, chosen as
/// the capture of a member's own turns chooses them, in transcript order. The
/// file is read through one checked handle. A transcript that is refused or
/// unreadable is an error: it is on disk, so it is not "membership only".
///
/// # Errors
///
/// [`HookError::TranscriptRefused`] or [`HookError::Read`].
pub fn past<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    session: &SessionId,
    path: &Path,
) -> Result<Option<Vec<message::NewMessage>>, HookError<E>> {
    let bytes = match ctx.transcripts.read_checked(provider, path, session) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok(None),
        Err(Unreadable::Refused(reason)) => {
            return Err(HookError::TranscriptRefused {
                id: session.clone(),
                reason,
            });
        }
        Err(Unreadable::Io(source)) => {
            return Err(HookError::Read {
                what: path.display().to_string(),
                source,
            });
        }
    };
    typed_turns(provider, session, &bytes).map(Some)
}

/// The typed turns in an already checked transcript of `session`, from its
/// first line, in transcript order. Lines of another session's copied history,
/// harness and `isMeta` lines, peer messages and scheduled prompts are not
/// turns. No model runs.
///
/// # Errors
///
/// [`HookError::TranscriptRefused`] when a Codex rollout does not parse.
pub fn typed_turns<E>(
    provider: Provider,
    session: &SessionId,
    bytes: &[u8],
) -> Result<Vec<message::NewMessage>, HookError<E>> {
    if provider == Provider::Codex {
        let rollout =
            crate::codex::parse(bytes).map_err(|reason| HookError::TranscriptRefused {
                id: session.clone(),
                reason,
            })?;
        return Ok(codex_messages(session, &rollout.typed));
    }
    Ok(crate::instruction::claude_group_turns(bytes, session))
}

fn save<E>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    session: &SessionId,
    messages: &[message::NewMessage],
) -> Result<Option<HookOutcome>, HookError<E>> {
    if messages.is_empty() {
        return Ok(None);
    }
    let config = ctx.store.read().tags().config()?.for_provider(provider);
    let redactor = ctx.redaction.redactor(&config).map_err(HookError::Port)?;
    let inserted = ctx
        .store
        .capture_group_turns(session, messages, &redactor)?;
    Ok(captured(session, inserted))
}

fn captured(session: &SessionId, inserted: usize) -> Option<HookOutcome> {
    (inserted > 0).then(|| HookOutcome {
        target: Some(format!("session:{session}")),
        error_code: None,
    })
}
