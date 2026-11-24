//! Restore a handoff for an identified predecessor, and resolve the shared
//! `derived-from` (`via: clear`) predecessor lookup:
//! [`scribe_core::cases::continuation`] for this process's provider.

use crate::app::Context;
use crate::output::HookReply;
use scribe_core::SessionId;
use scribe_core::cases::continuation as case;
use std::path::Path;

pub(in crate::commands) use scribe_core::cases::continuation::ClearedPredecessor;

/// Inspect `path` for opening `derived-from` (`via: clear`) ancestors of `session`.
pub(in crate::commands) fn cleared_predecessor(
    ctx: &Context,
    path: Option<&Path>,
    session: &SessionId,
) -> ClearedPredecessor {
    case::cleared_predecessor(&ctx.core(), ctx.provider(), path, session)
}

pub(super) fn at_start(
    ctx: &Context,
    child: &SessionId,
    predecessor: &ClearedPredecessor,
) -> Option<HookReply> {
    case::at_start(&ctx.core(), child, predecessor)
}
