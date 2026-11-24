//! Codex lifecycle adapter: the payload, the event check and `SessionStart`
//! here, then [`scribe_core::cases::codex::run`] (ORB-27707 step 3c). Only
//! host-attributed user.text messages can drive writes. A before-persistence
//! prompt is deferred to Stop, never upgraded from an unverified hook payload.
//! Cursors contain ids only, not private prompts.
//!
//! A `#scribe` marker inside a sentence (2_design.md §14) is echoed at submit
//! once the typed turn is verified in the rollout, and cataloged by Stop from
//! the rollout (`annotation.rs`), as under Claude Code.

use super::payload::{self, StopPayload};
use super::{group_add, session_end, session_start, user_prompt_submit};
use crate::{app::Context, error::CliError, output::Output};
use scribe_core::cases::codex::{self as case, CodexInput, CodexStop, has_marks, refused};
use serde_json::Value;

pub(super) fn run(event: &'static str, input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let output = ingest(event, input, ctx)?;
    if event != "UserPromptSubmit" {
        return Ok(output);
    }
    // A verified typed turn's markers are echoed here, once: Stop replays
    // turns through the prompt hook, which therefore does not echo them
    // under Codex. A refusal (unverified or wrong session) says Stop retries.
    let Output::Hook(mut reply) = output else {
        return Ok(output);
    };
    let prompt = payload::decode::<Value>(input)?["prompt"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if reply
        .outcomes
        .iter()
        .any(|o| o.error_code == Some("provenance_failed"))
        || !has_marks(&prompt)
    {
        return Ok(Output::Hook(reply));
    }
    let (texts, contexts): (Vec<String>, Vec<String>) =
        user_prompt_submit::mark_echoes(&prompt).into_iter().unzip();
    if texts.is_empty() {
        return Ok(Output::Hook(reply));
    }
    let join = |first: Option<String>, more: Vec<String>| {
        Some(first.into_iter().chain(more).collect::<Vec<_>>().join("\n"))
    };
    reply.message = join(reply.message.take(), texts);
    reply.context = join(reply.context.take(), contexts);
    Ok(Output::Hook(reply))
}

fn ingest(event: &'static str, input: &[u8], ctx: &Context) -> Result<Output, CliError> {
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
    let reply = case::run(
        &ctx.core(),
        CodexInput {
            event,
            payload: &value,
            provider: ctx.provider(),
            host: ctx.host(),
            session_end,
            stop,
            read_past: |provider, id: &_, path: &_| {
                group_add::read_past(&ctx.with_provider(provider), id, path)
            },
        },
    )?;
    Ok(Output::Hook(reply))
}
