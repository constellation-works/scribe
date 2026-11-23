//! Capture a member's typed turns from host-checked transcript data.

use super::{codex, payload};
use crate::{app::Context, error::CliError, output::HookOutcome};
use scribe_core::{SessionId, codex::TypedTurn, message, transcript};
use std::path::Path;

#[cfg(unix)]
mod claude;

#[cfg(all(test, unix))]
mod tests;

#[cfg(unix)]
pub(super) fn claude(
    ctx: &Context,
    session: &SessionId,
    path: &Path,
) -> Result<Option<HookOutcome>, CliError> {
    use scribe_core::{Redactor, instruction::ClaudeGroupTurns};

    // A concurrent prompt or Stop may commit between the read and write.
    // Retry from its newer snapshot without holding a lock over file I/O.
    for _ in 0..3 {
        let Some(before) = ctx.store().claude_group_capture(session)? else {
            return Ok(None);
        };
        let Some((mut file, meta)) =
            checked(payload::open_checked(ctx, path, session), path, session)?
        else {
            return Ok(None);
        };
        let read_error = |source| CliError::Read {
            what: path.display().to_string(),
            source,
        };
        let window = match claude::read_window(&mut file, &meta, before.progress()) {
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => continue,
            result => result.map_err(read_error)?,
        };
        let after = file.metadata().map_err(read_error)?;
        // Retry a truncation or same-size rewrite during the read.
        // Appends past the snapshot's end are for the next hook.
        if after.len() < meta.len()
            || (after.len() == meta.len()
                && after.modified().map_err(read_error)? != meta.modified().map_err(read_error)?)
        {
            continue;
        }
        let Some(window) = window else {
            return Ok(None);
        };
        let batch = ClaudeGroupTurns::parse(&window.bytes, session);
        let redactor = if batch.has_user_messages() {
            ctx.redactor(&ctx.config()?)?
        } else {
            Redactor::default()
        };
        if let Some(inserted) = ctx.store().capture_claude_group_turns(
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

#[cfg(not(unix))]
pub(super) fn claude(
    ctx: &Context,
    session: &SessionId,
    path: &Path,
) -> Result<Option<HookOutcome>, CliError> {
    if !ctx.store().captures_group_turns(session)? {
        return Ok(None);
    }
    let Some(bytes) = checked(payload::read_checked(ctx, path, session), path, session)? else {
        return Ok(None);
    };
    save(
        ctx,
        session,
        &scribe_core::instruction::claude_group_turns(&bytes, session),
    )
}

fn checked<T>(
    result: Result<Option<T>, payload::Unreadable>,
    path: &Path,
    session: &SessionId,
) -> Result<Option<T>, CliError> {
    match result {
        Ok(found) => Ok(found),
        Err(payload::Unreadable::Refused(reason)) => {
            tracing::warn!(%session, %reason, "group turn capture refused the transcript");
            Ok(None)
        }
        Err(payload::Unreadable::Io(source)) => Err(CliError::Read {
            what: path.display().to_string(),
            source,
        }),
    }
}

pub(super) fn codex(
    ctx: &Context,
    session: &SessionId,
    turns: &[TypedTurn],
) -> Result<Option<HookOutcome>, CliError> {
    if !ctx.store().captures_group_turns(session)? {
        return Ok(None);
    }
    let messages: Vec<_> = turns
        .iter()
        .filter_map(|turn| {
            let input = &turn.message;
            if codex::is_control(&input.text()) {
                return None;
            }
            message::typed_turn(session, input, &transcript::user_turn(input)?.own)
        })
        .collect();
    save(ctx, session, &messages)
}

fn save(
    ctx: &Context,
    session: &SessionId,
    messages: &[message::NewMessage],
) -> Result<Option<HookOutcome>, CliError> {
    if messages.is_empty() {
        return Ok(None);
    }
    let redactor = ctx.redactor(&ctx.config()?)?;
    let inserted = ctx
        .store()
        .capture_group_turns(session, messages, &redactor)?;
    Ok(captured(session, inserted))
}

fn captured(session: &SessionId, inserted: usize) -> Option<HookOutcome> {
    (inserted > 0).then(|| HookOutcome {
        target: Some(format!("session:{session}")),
        error_code: None,
    })
}
