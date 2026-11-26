//! Restore a handoff for an identified predecessor or the user's explicit
//! selection (ORB-27707 step 3b). The predecessor lookup is the session
//! app's.

use super::reply::HookReply;
use crate::SessionId;
use crate::apps::session::{self, ClearedPredecessor, HandoffSelection};
use crate::runtime::Ctx;
use crate::runtime::audit::HookOutcome;

/// Restore the handoff `selection` names into `child`: its body as context,
/// one outcome when it changed anything, or the refusal.
#[must_use]
pub fn restore<E>(
    ctx: &Ctx<'_, E>,
    event: &'static str,
    child: &SessionId,
    selection: HandoffSelection<'_>,
) -> HookReply {
    let mut reply = HookReply {
        event,
        message: None,
        context: None,
        block: None,
        outcomes: Vec::new(),
    };
    match session::continue_handoff(ctx.store, child, selection, ctx.clock.now()) {
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

/// The restoration a `SessionStart` makes for its cleared predecessor:
/// none, the one predecessor's handoff, or the ambiguity.
#[must_use]
pub fn at_start<E>(
    ctx: &Ctx<'_, E>,
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
