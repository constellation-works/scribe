//! SessionStart: the payload, then [`scribe_core::cases::session_start::run`],
//! which reports open proposals and unpicked actions, restores a cleared
//! session's handoff and seats it, and gives an interactive session the
//! ordinary brief.

use super::payload::{self, SessionStartPayload};
use crate::app::Context;
use crate::error::CliError;
use crate::output::Output;
use scribe_core::cases::session_start::{self as case, SessionStartInput};

/// Run the hook on its stdin payload.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let payload: SessionStartPayload = payload::decode(input)?;
    let session = payload::session(&payload.session_id)?;
    let reply = case::run(
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
    Ok(Output::Hook(reply))
}
