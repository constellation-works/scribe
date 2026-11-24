//! Capture a member's typed turns from host-checked transcript data:
//! the arguments, then the group app's turn capture.

use scribe_core::apps::group;
use scribe_core::composition::Context;
use scribe_core::runtime::errors::CommandError;
use scribe_core::{SessionId, message};
use std::path::Path;

/// The typed turns of a past session whose transcript is at `path`
/// ([`group::past_turns`]).
pub(super) fn past(
    ctx: &Context,
    session: &SessionId,
    path: &Path,
) -> Result<Option<Vec<message::NewMessage>>, CommandError> {
    Ok(group::past_turns(
        &ctx.core(),
        ctx.provider(),
        session,
        path,
    )?)
}
