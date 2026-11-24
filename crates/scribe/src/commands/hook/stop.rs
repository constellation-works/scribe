//! `scribe hook stop`: check the transcript, fill in cite message ids
//! (§4.1), catalog the turn's `#scribe` markers, and mark a switched-on
//! session's pass due (§5.2).
//!
//! The `#scribe` markers the user typed are cataloged (`scribe_core::cases::annotation`, §14):
//! the only model call this hook makes, one per marked turn, bounded. A
//! marker given up after its last failed call is echoed to the user and, with
//! its line, to the agent (`additionalContext`). A transcript that fails the
//! provenance check is echoed once and stops the hook: nothing is filled,
//! cataloged or marked due.
//!
//! `UserPromptSubmit` runs before Claude Code writes the prompt to the
//! transcript, so an entry's cite starts with `message: null` and the hash
//! of the prompt. This hook finds the user message whose text has that hash
//! and records its uuid. When several messages match (the same prompt sent
//! twice), the one written closest to the cite's time is chosen.
//!
//! An open group member's transcript progress can advance independently of
//! cite filling or marker cataloging.
//!
//! Then the cadence: for a session that is on and not already due, the pass
//! is due once `[catalog] every_minutes` have passed since the last pass
//! (the record's `updated`, else when the session was switched on) or
//! `every_turns` of the user's turns have arrived since the record's
//! `scribed_through`. Minutes are checked first, so the transcript is read
//! only to count turns for cadence. Markers are cataloged also for a session
//! that is off or was never switched on.

use super::payload::{self, StopPayload};
use crate::app::Context;
use crate::error::CliError;
use crate::output::Output;
use scribe_core::cases::stop::{self as case, StopInput};

/// Run the hook on its stdin payload.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let payload: StopPayload = payload::decode(input)?;
    let session = payload::session(&payload.session_id)?;
    let reply = case::run(
        &ctx.core(),
        StopInput {
            session: &session,
            transcript_path: &payload.transcript_path,
            cwd: payload.cwd,
            provider: ctx.provider(),
            host: ctx.host(),
        },
    )?;
    Ok(Output::Hook(reply))
}
