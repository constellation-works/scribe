//! Codex lifecycle adapter: the payload, the event check and `SessionStart`
//! here, then the hook app's `Codex` command (ORB-27707 step 3c), which
//! also echoes a verified typed turn's `#scribe` markers. Only
//! host-attributed user.text messages can drive writes. A before-persistence
//! prompt is deferred to Stop, never upgraded from an unverified hook payload.
//! Cursors contain ids only, not private prompts.
//!
//! A `#scribe` marker inside a sentence (2_design.md §14) is echoed at submit
//! once the typed turn is verified in the rollout, and cataloged by Stop from
//! the rollout (`annotation.rs`), as under Claude Code.

use super::payload::{self, StopPayload};
use super::{group_add, session_end, session_start};
use crate::{error::CliError, output::Output};
use scribe_core::apps::hook::{CodexInput, CodexStop, Command, exec, refused};
use scribe_core::composition::Context;
use serde_json::Value;

pub(super) fn run(event: &'static str, input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let value: Value = payload::decode(input)?;
    if value["hook_event_name"] != event {
        return Ok(Output::Hook(refused(
            event,
            "Codex hook_event_name is missing or does not match this entry point",
        )));
    }
    if event == "SessionStart" {
        return session_start::run(input, ctx);
    }
    // Decoded here, raised by the case where the old adapter decoded them.
    let session_end = (event == "SessionEnd").then(|| session_end::session(input));
    let stop = (event == "Stop").then(|| {
        let payload: StopPayload = payload::decode(input)?;
        Ok(CodexStop {
            session: payload::session(&payload.session_id)?,
            transcript_path: payload.transcript_path,
            cwd: payload.cwd,
        })
    });
    let mut read_past =
        |provider, id: &_, path: &_| group_add::read_past(&ctx.with_provider(provider), id, path);
    let command = Command::Codex(CodexInput {
        event,
        payload: &value,
        provider: ctx.provider(),
        host: ctx.host(),
        session_end,
        stop,
        read_past: &mut read_past,
    });
    Ok(super::replied(exec(&ctx.runtime(), command)?))
}
