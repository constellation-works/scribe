//! `scribe hook session-start`: tell Daniel how many proposals wait for him
//! (docs/design/scribe/2_design.md §7), and how many action entries no
//! consumer has picked up (§4.6).
//!
//! Read-only and model-free: it reads `proposals.json` and, when any
//! proposal (including a session-pass candidate) is open, prints one `systemMessage` line for Daniel, `scribe: 3
//! proposals to review — scribe proposal list`. It then counts the active
//! entries under action tags older than `action::STUCK_AFTER` with no
//! pickup in `pickups.json` and, when there are any, adds one line,
//! `scribe: 2 action entries not picked up after 1 hour (standard 1,
//! task 1) — scribe entry list --tag <tag> --status active`. The agent is not
//! told: settling is Daniel's, in a typed turn. Nothing to report, no store
//! yet, or a store that cannot be read (logged) prints nothing, so a
//! session always starts. It writes nothing and is not audited.

use super::payload::{self, SessionStartPayload};
use crate::app::Context;
use crate::error::CliError;
use crate::output::{HookReply, Output};
use scribe_core::{ProposalState, TagName};
use std::collections::BTreeMap;

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
    let proposals = match open {
        0 => None,
        1 => Some(String::from(
            "scribe: 1 proposal to review — scribe proposal list",
        )),
        n => Some(format!(
            "scribe: {n} proposals to review — scribe proposal list"
        )),
    };
    let stuck = match ctx.store().stuck_actions(ctx.now()) {
        Ok(counts) => stuck_line(&counts),
        Err(err) => {
            tracing::warn!(%err, "cannot read the action entries; not reporting them");
            None
        }
    };
    let lines: Vec<String> = proposals.into_iter().chain(stuck).collect();
    let message = (!lines.is_empty()).then(|| lines.join("\n"));
    Ok(Output::Hook(HookReply {
        event: EVENT,
        message,
        context: None,
        block: None,
        outcomes: Vec::new(),
    }))
}

/// The stuck-entries line, or `None` when there are none.
fn stuck_line(counts: &BTreeMap<TagName, usize>) -> Option<String> {
    let total: usize = counts.values().sum();
    if total == 0 {
        return None;
    }
    let hours = scribe_core::action::STUCK_AFTER.whole_hours();
    let age = if hours == 1 {
        String::from("1 hour")
    } else {
        format!("{hours} hours")
    };
    let per_tag: Vec<String> = counts.iter().map(|(tag, n)| format!("{tag} {n}")).collect();
    let noun = if total == 1 { "entry" } else { "entries" };
    Some(format!(
        "scribe: {total} action {noun} not picked up after {age} ({}) — scribe entry list --tag <tag> --status active",
        per_tag.join(", ")
    ))
}
