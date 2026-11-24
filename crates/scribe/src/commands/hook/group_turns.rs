//! Capture a member's typed turns from host-checked transcript data: the
//! group app's turn capture.

use scribe_core::CommandError;
use scribe_core::apps::group;
use scribe_core::composition::Context;
use scribe_core::{SessionId, message};
use std::path::Path;

/// The typed turns of a past session whose transcript is at `path`
/// ([`group::past_turns_of`]).
pub(super) fn past(
    ctx: &Context,
    session: &SessionId,
    path: &Path,
) -> Result<Option<Vec<message::NewMessage>>, CommandError> {
    group::past_turns_of(ctx, session, path)
}
