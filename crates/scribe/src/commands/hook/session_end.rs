//! `scribe hook session-end`: the payload, then
//! the hook app's `SessionEnd`. The session ended, so its last
//! pass is due and final (docs/design/scribe/2_design.md §5.2).
//!
//! Model-free and cheap: it reads `control.json` and, only for a session
//! that is on, sets `due` and `final` under the store lock. A session that
//! was never switched on, or is off, is left alone: nothing is read beyond
//! `control.json`, nothing is written and nothing is audited. The pass
//! itself runs later from `scribe session pass`. It prints nothing.

use super::payload::{self, SessionEndPayload};
use crate::app::Context;
use crate::error::CliError;
use crate::output::Output;
use scribe_core::SessionId;
use scribe_core::apps::hook::{Command, exec};

/// Run the hook on its stdin payload.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let command = Command::SessionEnd(session(input)?);
    Ok(super::replied(exec(&ctx.runtime(), command)?))
}

/// The session a `SessionEnd` payload names.
pub(super) fn session(input: &[u8]) -> Result<SessionId, CliError> {
    let payload: SessionEndPayload = payload::decode(input)?;
    payload::session(&payload.session_id)
}
