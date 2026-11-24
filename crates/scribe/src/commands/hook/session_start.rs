//! SessionStart: the payload, then [`scribe_core::cases::session_start::run`],
//! which reports open proposals and unpicked actions, restores a cleared
//! session's handoff and seats it. An interactive Claude session also
//! receives the ordinary brief, appended here after the case's context.

use super::payload::{self, SessionStartPayload};
use super::project;
use crate::app::Context;
use crate::error::CliError;
use crate::output::Output;
use scribe_core::SessionId;
use scribe_core::cases::session_start::{self as case, SessionStartInput};
use std::path::Path;

/// Run the hook on its stdin payload.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let payload: SessionStartPayload = payload::decode(input)?;
    let session = payload::session(&payload.session_id)?;
    let mut reply = case::run(
        &ctx.core(),
        &SessionStartInput {
            session: &session,
            transcript_path: payload.transcript_path.as_deref(),
            source: payload.source.as_deref(),
            cwd: payload.cwd.as_deref(),
            interactive: ctx.interactive(),
            provider: ctx.provider(),
        },
    );
    let brief = ctx
        .interactive()
        .then(|| brief_text(ctx, &session, payload.cwd.as_deref()))
        .flatten();
    let context_parts: Vec<String> = reply.context.take().into_iter().chain(brief).collect();
    reply.context = (!context_parts.is_empty()).then(|| context_parts.join("\n\n"));
    Ok(Output::Hook(reply))
}

/// The memory block's text, or `None` when memory holds nothing or the
/// store cannot be read (logged, so the session still starts).
fn memory_block(ctx: &Context) -> Option<String> {
    match crate::commands::memory::block(ctx) {
        Ok(block) => {
            for warning in &block.warnings {
                tracing::warn!("{warning}");
            }
            let empty =
                block.profile.is_none() && block.now.is_empty() && block.milestones.is_empty();
            (!empty).then_some(block.text)
        }
        Err(err) => {
            tracing::warn!(%err, "cannot read the memory; not injecting the memory block");
            None
        }
    }
}

/// The brief for this session's project, or `None` when it has no memory,
/// last group, session or claim, or the store cannot be read (logged, so the
/// session still starts). Memory that cannot be read is left out (logged by
/// [`memory_block`]); the rest is still given. With no `cwd`,
/// every project's sessions by date alone.
fn brief_text(ctx: &Context, session: &SessionId, cwd: Option<&str>) -> Option<String> {
    let project = cwd
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .map(|cwd| project::of(Path::new(cwd)));
    let memory = memory_block(ctx);
    match crate::commands::brief::brief(
        ctx,
        memory,
        project.as_ref(),
        Some(session),
        scribe_core::brief::DEFAULT_SESSIONS,
    ) {
        Ok(brief)
            if brief.memory.is_none()
                && brief.last_group.is_none()
                && brief.sessions.is_empty()
                && brief.claims.is_empty() =>
        {
            None
        }
        Ok(brief) => Some(brief.text_with_ids(&crate::commands::brief::brief_ids(ctx, &brief))),
        Err(err) => {
            tracing::warn!(%err, "cannot read the sessions or proposals; not injecting the brief");
            None
        }
    }
}
