//! Capture a member's typed turns from host-checked transcript data.

use super::{codex, payload};
use crate::{app::Context, error::CliError, output::HookOutcome};
use scribe_core::{SessionId, codex::TypedTurn, instruction, message, transcript};
use std::path::Path;

pub(super) fn claude(
    ctx: &Context,
    session: &SessionId,
    path: &Path,
) -> Result<Option<HookOutcome>, CliError> {
    if !ctx.store().captures_group_turns(session)? {
        return Ok(None);
    }
    let bytes = match payload::read_checked(ctx, path, session) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok(None),
        Err(payload::Unreadable::Refused(reason)) => {
            tracing::warn!(%session, %reason, "group turn capture refused the transcript");
            return Ok(None);
        }
        Err(payload::Unreadable::Io(source)) => {
            return Err(CliError::Read {
                what: path.display().to_string(),
                source,
            });
        }
    };
    save(
        ctx,
        session,
        &instruction::claude_group_turns(&bytes, session),
    )
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
    Ok((inserted > 0).then(|| HookOutcome {
        target: Some(format!("session:{session}")),
        error_code: None,
    }))
}
