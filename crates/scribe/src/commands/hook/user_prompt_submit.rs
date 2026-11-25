//! `scribe hook user-prompt-submit`: the payload, then
//! the hook app's `UserPromptSubmit`, which echoes the `#scribe` markers of a typed
//! prompt and apply its settling commands (docs/design/scribe/2_design.md
//! §4.4, §14).
//!
//! A turn that is not plainly typed by the user (an agent or harness wrapper
//! such as `<agent-message>`, `<task-notification>`,
//! `<side-sessions-event>` or a scheduled/loop wrapper, or a non-plain
//! payload origin field; see the core's hook payload docs) is skipped at
//! once: it echoes nothing, settles nothing and does not run `/scribe`.
//!
//! Every `#scribe` the user typed, inside a sentence or opening a line
//! (`#scribe idea: …`, `#scribe add this to preferences`), is a marker
//! (§14.6): this hook writes nothing and calls no model for it, it only
//! echoes that the marker is noted; the Stop of this turn catalogs it
//! (the hook app's annotation). Under Codex the hook app's Codex adapter echoes them
//! instead, once it has verified the typed turn.
//!
//! A settling line (`#scribe approve|reject <p-id>`,
//! `#scribe retire|forget <entry-id>`, form 4) is a whole-line command, not
//! a marker: after the transcript check it goes to the hook app's `proposal_command`
//! (or `issue_command.rs` for an issue id) and is echoed in its place.
//!
//! A prompt whose first line is `/scribe on`, `/scribe off` or
//! `/scribe status` is not a turn: the hook app's `scribe_command` switches the session
//! and blocks it (§5.1), and its `#scribe` lines, if any, are not read.
//!
//! The reply tells the user (`systemMessage`) one echo per settling line and
//! one for the markers. The main agent (`additionalContext`) gets the same
//! echoes, each with a note: applied lines say no action is needed,
//! refused or failed lines say they must be relayed to the user, and markers
//! say Stop hands them to the agent after the reply, so the agent records
//! nothing for them before then. The hook never blocks the
//! prompt: a refusal or a store failure is a reply, not an error exit, so
//! the rest of the prompt still reaches the agent.

#![cfg(test)]

use super::group_add;
use crate::error::CliError;
use crate::output::Output;
use scribe_core::apps::hook::{Event, exec_unaudited};
use scribe_core::composition::Context;

/// Run the hook on `input`, unaudited, as the tests drive it.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let mut read_past =
        |provider, id: &_, path: &_| group_add::read_past(&ctx.with_provider(provider), id, path);
    let event = Event::UserPromptSubmit {
        input: Ok(input.to_vec()),
        read_past: &mut read_past,
    };
    Ok(super::replied(exec_unaudited(ctx, event)?))
}
