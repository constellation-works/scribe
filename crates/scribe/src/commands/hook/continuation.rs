//! Restore a handoff for an identified predecessor or the user's explicit selection,
//! and resolve the shared `derived-from` (`via: clear`) predecessor lookup.

use super::payload;
use crate::app::Context;
use crate::output::{HookOutcome, HookReply};
use scribe_core::{HandoffSelection, Provider, SessionId};
use std::path::Path;

/// What a Claude Code transcript's opening `derived-from` metadata says about
/// which session(s) a new transcript was cleared from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::commands) enum ClearedPredecessor {
    /// Not Claude Code, path missing, file absent/unreadable, or no `via: clear` entry.
    None,
    /// One distinct `via: clear` predecessor.
    One(SessionId),
    /// Multiple distinct `via: clear` predecessors.
    Multiple(Vec<SessionId>),
}

/// Inspect `path` for opening `derived-from` (`via: clear`) ancestors of `session`.
/// Returns `None` when the transcript does not yet exist or cannot be read.
pub(super) fn try_cleared_predecessor(
    ctx: &Context,
    path: &Path,
    session: &SessionId,
) -> Option<ClearedPredecessor> {
    if ctx.provider() != Provider::ClaudeCode {
        return Some(ClearedPredecessor::None);
    }
    let header = match payload::claude_header(ctx, path, session) {
        Ok(Some(header)) => header,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!(?err, "cannot read session ancestry");
            return None;
        }
    };
    let mut parents = scribe_core::transcript::cleared_sessions(&header);
    Some(match parents.len() {
        0 => ClearedPredecessor::None,
        1 => ClearedPredecessor::One(parents.swap_remove(0)),
        _ => ClearedPredecessor::Multiple(parents),
    })
}

/// Inspect `path` for opening `derived-from` (`via: clear`) ancestors of `session`.
pub(in crate::commands) fn cleared_predecessor(
    ctx: &Context,
    path: Option<&Path>,
    session: &SessionId,
) -> ClearedPredecessor {
    let Some(path) = path else {
        return ClearedPredecessor::None;
    };
    try_cleared_predecessor(ctx, path, session).unwrap_or(ClearedPredecessor::None)
}

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
    child: &SessionId,
    predecessor: &ClearedPredecessor,
) -> Option<HookReply> {
    match predecessor {
        ClearedPredecessor::None => None,
        ClearedPredecessor::One(parent) => Some(restore(
            ctx,
            "SessionStart",
            child,
            HandoffSelection::Predecessor(parent),
        )),
        ClearedPredecessor::Multiple(parents) => Some(HookReply {
            event: "SessionStart",
            message: Some(ambiguous(parents)),
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
