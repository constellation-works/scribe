//! SessionStart: the payload, then the hook app's `SessionStart`,
//! which reports open proposals and unpicked actions, restores a cleared
//! session's handoff and seats it, and gives an interactive session the
//! ordinary brief.

use super::payload::{self, SessionStartPayload};
use crate::error::CliError;
use crate::output::Output;
use scribe_core::apps::hook::{Command, SessionStartInput, exec};
use scribe_core::composition::Context;

/// Run the hook on its stdin payload.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let payload: SessionStartPayload = payload::decode(input)?;
    let session = payload::session(&payload.session_id)?;
    let command = Command::SessionStart(SessionStartInput {
        session: &session,
        transcript_path: payload.transcript_path.as_deref(),
        source: payload.source.as_deref(),
        cwd: payload.cwd.as_deref(),
        interactive: ctx.interactive(),
        provider: ctx.provider(),
    });
    Ok(super::replied(exec(&ctx.runtime(), command)?))
}
