//! `scribe hook session-start`: tell Daniel how many proposals wait for him
//! (docs/design/scribe/2_design.md §7).
//!
//! Read-only and model-free: it reads `proposals.json` and, when any
//! proposal is open, prints one `systemMessage` for Daniel, `scribe: 3
//! proposals to review — scribe proposal list`. The agent is not told:
//! settling is Daniel's, in a typed turn. Nothing open, no store yet, or a
//! store that cannot be read (logged) prints nothing, so a session always
//! starts. It writes nothing and is not audited.

use super::payload::{self, SessionStartPayload};
use crate::app::Context;
use crate::error::CliError;
use crate::output::{HookReply, Output};
use scribe_core::ProposalState;

/// The hook event this runs for.
const EVENT: &str = "SessionStart";

/// Run the hook on its stdin payload.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let payload: SessionStartPayload = payload::decode(input)?;
    payload::session(&payload.session_id)?;
    let open = match ctx.store().proposals() {
        Ok(proposals) => proposals
            .iter()
            .filter(|p| p.state == ProposalState::Open)
            .count(),
        Err(err) => {
            tracing::warn!(%err, "cannot read the proposals; not reporting them");
            0
        }
    };
    let message = match open {
        0 => None,
        1 => Some(String::from(
            "scribe: 1 proposal to review — scribe proposal list",
        )),
        n => Some(format!(
            "scribe: {n} proposals to review — scribe proposal list"
        )),
    };
    Ok(Output::Hook(HookReply {
        event: EVENT,
        message,
        context: None,
        block: None,
        outcomes: Vec::new(),
    }))
}
