//! The `SessionStart` group seat (ORB-27707 step 3b): a session keeps the
//! seat it holds or inherits its cleared predecessor's. The prompt hook's
//! deferred ancestry check seats a session here too.

use crate::SessionId;
use crate::cases::Ctx;
use crate::cases::continuation::ClearedPredecessor;
use crate::cases::group::GroupVerbs;
use crate::cases::hook::HookOutcome;
use crate::group::SeatStart;

/// What a session's group seat at its start adds to the hook's reply.
#[derive(Debug, Default)]
pub struct SeatReply {
    /// For the agent.
    pub context: Option<String>,
    /// For the user.
    pub message: Option<String>,
    /// For the audit log.
    pub outcomes: Vec<HookOutcome>,
}

/// Seat `child` at its start: keep the seat it holds, or inherit its one
/// cleared predecessor's, and say which.
#[must_use]
pub fn seat_at_start<E>(
    ctx: &Ctx<'_, E>,
    child: &SessionId,
    predecessor: &ClearedPredecessor,
) -> SeatReply {
    let parent = match predecessor {
        ClearedPredecessor::One(parent) => Some(parent),
        ClearedPredecessor::None | ClearedPredecessor::Multiple(_) => None,
    };
    match ctx.store.inherit_seat(child, parent) {
        Ok(SeatStart::NoOpenGroups) => SeatReply::default(),
        Ok(SeatStart::AlreadySeated(hold)) => SeatReply {
            context: Some(format!(
                "scribe: session {child} holds seat {} ({}) in {}",
                hold.seat, hold.role, hold.group
            )),
            message: None,
            outcomes: Vec::new(),
        },
        Ok(SeatStart::Replaced) => SeatReply {
            context: Some(format!(
                "scribe: session {child} was replaced on its group seat and holds none now; nothing inherited"
            )),
            message: None,
            outcomes: Vec::new(),
        },
        Ok(SeatStart::Inherited { hold, from }) => SeatReply {
            context: Some(format!(
                "scribe: session {child} holds seat {} ({}) in {} (inherited from {from})",
                hold.seat, hold.role, hold.group
            )),
            message: None,
            outcomes: vec![HookOutcome {
                target: Some(hold.group),
                error_code: None,
            }],
        },
        Ok(SeatStart::Unjoined) => {
            let reason = match predecessor {
                ClearedPredecessor::None => {
                    "scribe: no predecessor line in this transcript; no group seat was inherited — re-run `scribe group join` if your brief names a group"
                        .to_owned()
                }
                ClearedPredecessor::One(parent) => format!(
                    "scribe: predecessor {parent} holds no open group seat; nothing inherited — re-run `scribe group join` if your brief names a group"
                ),
                ClearedPredecessor::Multiple(_) => {
                    "scribe: multiple cleared predecessors in this transcript; no group seat was inherited — re-run `scribe group join` if your brief names a group"
                        .to_owned()
                }
            };
            SeatReply {
                context: Some(reason),
                message: None,
                outcomes: Vec::new(),
            }
        }
        Err(err) => {
            tracing::warn!(%err, "cannot inherit group seat");
            SeatReply {
                context: Some(format!(
                    "scribe: could not inherit group seat ({err}) — re-run `scribe group join` if your brief names a group"
                )),
                message: Some(format!("scribe: could not inherit group seat — {err}")),
                outcomes: vec![HookOutcome {
                    target: Some(format!("session:{child}")),
                    error_code: Some(err.code()),
                }],
            }
        }
    }
}
