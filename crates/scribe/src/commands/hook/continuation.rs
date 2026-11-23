//! Restore a handoff for an identified predecessor or the user's explicit selection.

use super::payload::{self, SessionStartPayload};
use crate::app::Context;
use crate::output::{HookOutcome, HookReply};
use scribe_core::{HandoffSelection, Provider, SessionId};

pub(super) fn restore(
    ctx: &Context,
    event: &'static str,
    child: &SessionId,
    selection: HandoffSelection<'_>,
) -> HookReply {
    let mut reply = HookReply {
        event,
        context: None,
        message: None,
        block: None,
        outcomes: Vec::new(),
    };
    match ctx.store().continue_handoff(child, selection, ctx.now()) {
        Ok(Some((body, changed))) => {
            reply.context = Some(body);
            if changed {
                reply.outcomes.push(HookOutcome {
                    target: Some(format!("session:{child}")),
                    error_code: None,
                });
            }
        }
        Ok(None) => {}
        Err(err) => {
            let reason = format!("scribe: could not continue this session — {err}");
            match selection {
                HandoffSelection::Predecessor(_) => reply.message = Some(reason),
                HandoffSelection::Named(_) => reply.block = Some(reason),
            }
            reply.outcomes.push(HookOutcome {
                target: Some(format!("session:{child}")),
                error_code: Some(err.code()),
            });
        }
    }
    reply
}

pub(super) fn at_start(
    ctx: &Context,
    payload: &SessionStartPayload,
    child: &SessionId,
) -> Option<HookReply> {
    if !ctx.interactive()
        || ctx.provider() != Provider::ClaudeCode
        || !matches!(payload.source.as_deref(), None | Some("startup" | "clear"))
    {
        return None;
    }
    let path = payload.transcript_path.as_deref()?;
    let header = match payload::claude_header(ctx, path, child) {
        Ok(header) => header?,
        Err(err) => {
            tracing::warn!(?err, "cannot read session ancestry");
            return None;
        }
    };
    let parents = scribe_core::transcript::cleared_sessions(&header);
    match parents.as_slice() {
        [] => None,
        [parent] => Some(restore(
            ctx,
            "SessionStart",
            child,
            HandoffSelection::Predecessor(parent),
        )),
        _ => Some(HookReply {
            event: "SessionStart",
            message: Some(ambiguous(&parents)),
            context: None,
            block: None,
            outcomes: Vec::new(),
        }),
    }
}

fn ambiguous(ids: &[SessionId]) -> String {
    let ids = ids
        .iter()
        .map(SessionId::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "scribe: possible predecessors {ids} — type /scribe continue <handoff-id> to choose a handoff"
    )
}
