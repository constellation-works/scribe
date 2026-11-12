//! Codex lifecycle adapter. Only host-attributed user.text messages can drive
//! writes. A before-persistence prompt is deferred to Stop, never upgraded from
//! an unverified hook payload. Cursors contain ids only, not private prompts.

use super::{payload, session_end, session_start, stop, user_prompt_submit};
use crate::{
    app::Context,
    error::CliError,
    output::{HookOutcome, HookReply, Output},
};
use scribe_core::{Provider, capture, codex::TypedTurn, control};
use serde_json::{Value, json};
use std::path::Path;

/// Normalize only exact typed skill-control invocations. Injected skill text
/// never reaches this function: host content provenance is checked first.
fn command_text(original: &str) -> String {
    let text = original.trim();
    let args = text.strip_prefix("$scribe ").or_else(|| {
        let (label, rest) = text.split_once("](")?;
        if !matches!(label, "[$scribe]" | "[$scribe:scribe]")
            && !matches!(label, "[$scribe" | "[$scribe:scribe")
        {
            return None;
        }
        let (path, args) = rest.split_once(") ")?;
        if !path.ends_with("/scribe/SKILL.md") {
            return None;
        }
        Some(args)
    });
    if let Some(args) = args.filter(|s| !s.contains('\n')) {
        let candidate = format!("/scribe {args}");
        if matches!(
            control::parse(&candidate),
            Some(
                control::Switch::On
                    | control::Switch::Off
                    | control::Switch::Status
                    | control::Switch::Link { .. }
            )
        ) {
            return candidate;
        }
    }
    original.to_owned()
}

fn relevant(turn: &TypedTurn) -> bool {
    let text = command_text(&turn.message.text());
    control::parse(&text).is_some() || !capture::lines(&text).is_empty()
}

pub(super) fn run(event: &'static str, input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let value: Value = payload::decode(input)?;
    if value["hook_event_name"] != event {
        return Ok(refused(
            event,
            "Codex hook_event_name is missing or does not match this entry point",
        ));
    }
    if event == "SessionStart" {
        return session_start::run(input, ctx);
    }
    let Some(raw_id) = value["session_id"].as_str() else {
        return Ok(refused(
            event,
            "Codex hook requires session_id; nothing changed",
        ));
    };
    let id = match payload::session(raw_id) {
        Ok(id) => id,
        Err(_) => return Ok(refused(event, "invalid Codex session_id; nothing changed")),
    };
    let Some(path) = value["transcript_path"].as_str().map(Path::new) else {
        return Ok(refused(
            event,
            "Codex supplied no transcript; recording is unavailable. Use a supported desktop user thread and review scribe hooks",
        ));
    };
    match payload::check(ctx, path, &id) {
        Ok(payload::Transcript::Present) => {}
        Ok(payload::Transcript::Missing) => {
            return Ok(refused(
                event,
                "waiting for Codex transcript persistence; nothing recorded yet; Stop will retry verified inputs",
            ));
        }
        Err(reason) => return Ok(refused(event, &reason)),
    }
    let bytes = payload::read_for(ctx, path).map_err(|source| CliError::Read {
        what: path.display().to_string(),
        source,
    })?;
    let rollout = match scribe_core::codex::parse(&bytes) {
        Ok(rollout) => rollout,
        Err(reason) => return Ok(refused(event, &reason)),
    };
    if rollout.session != id {
        return Ok(refused(
            event,
            "opened transcript session identity does not match the hook session_id",
        ));
    }
    if ctx
        .store()
        .control()?
        .get(&id)
        .is_some_and(|c| c.provider != Provider::Codex)
    {
        return Ok(refused(
            event,
            "session id already belongs to another provider; nothing changed",
        ));
    }
    if event == "SessionEnd" {
        return session_end::run(input, ctx);
    }
    let Some(turn_id) = value["turn_id"].as_str().filter(|s| !s.is_empty()) else {
        return Ok(refused(
            event,
            "Codex supplied no turn_id; typed input cannot be verified",
        ));
    };
    if event == "UserPromptSubmit" {
        let Some(prompt) = value["prompt"].as_str() else {
            return Ok(refused(event, "Codex supplied no prompt"));
        };
        if !rollout
            .typed
            .iter()
            .any(|t| t.turn_id == turn_id && t.message.text() == prompt)
        {
            if control::parse(&command_text(prompt)).is_none() && capture::lines(prompt).is_empty()
            {
                return Ok(silent(event));
            }
            return Ok(refused(
                event,
                "typed input not yet verified in the host transcript; nothing recorded yet; Stop will retry. Agent or skill-injected text cannot activate scribe",
            ));
        }
    }
    let candidates: Vec<_> = rollout.typed.iter().filter(|t| relevant(t)).collect();
    if candidates.is_empty() {
        return if event == "Stop" {
            stop::run(input, ctx)
        } else {
            Ok(silent(event))
        };
    }
    let before = ctx.store().control()?;
    if before
        .codex_cursors
        .get(&id)
        .is_some_and(|cursor| candidates.last().is_some_and(|t| &t.message.uuid == cursor))
    {
        return if event == "Stop" {
            stop::run(input, ctx)
        } else {
            Ok(silent(event))
        };
    }
    let _ingestion = ctx.store().lock_codex_ingestion(&id)?;
    let state = ctx.store().control()?;
    let cursor = state.codex_cursors.get(&id);
    let start = match cursor {
        Some(cursor) => match candidates.iter().position(|t| &t.message.uuid == cursor) {
            Some(n) => n + 1,
            None => {
                return Ok(refused(
                    event,
                    "Codex ingestion cursor disappeared; transcript was rewritten. Nothing replayed; inspect the session before resuming",
                ));
            }
        },
        None => candidates
            .iter()
            .position(|t| t.turn_id == turn_id)
            .unwrap_or(candidates.len()),
    };
    let mut replies = Vec::new();
    let mut outcomes = Vec::new();
    for turn in &candidates[start..] {
        if event == "UserPromptSubmit"
            && (turn.turn_id != turn_id
                || turn.message.text() != value["prompt"].as_str().unwrap_or_default())
        {
            break;
        }
        let text = command_text(&turn.message.text());
        let adapted = serde_json::to_vec(&json!({"session_id": id, "transcript_path": path, "cwd": rollout.cwd, "prompt": text, "source":"typed"})).map_err(|e| CliError::Usage(e.to_string()))?;
        let output = user_prompt_submit::run(&adapted, ctx)?;
        if let Output::Hook(reply) = output {
            let failed = reply.outcomes.iter().any(|o| o.error_code.is_some());
            replies.extend(reply.message.or(reply.block));
            outcomes.extend(reply.outcomes);
            if failed
                && outcomes.iter().any(|o| {
                    matches!(
                        o.error_code,
                        Some("io_error" | "store_locked" | "redact_env_unset" | "config_invalid")
                    )
                })
            {
                break;
            }
            ctx.store().update_control(|state| {
                if let Some(switch) = state.sessions.get_mut(&id) {
                    switch.provider = Provider::Codex;
                }
                state
                    .codex_cursors
                    .insert(id.clone(), turn.message.uuid.clone());
            })?;
            outcomes.push(HookOutcome {
                target: Some(format!("session:{id}")),
                error_code: None,
            });
        }
    }
    if event == "Stop"
        && let Output::Hook(reply) = stop::run(input, ctx)?
    {
        outcomes.extend(reply.outcomes);
    }
    let message = (!replies.is_empty()).then(|| replies.join("\n"));
    // Codex Stop permits additionalContext JSON; no plain stdout, and no
    // blocking decision that would accidentally ask the model to continue.
    Ok(Output::Hook(HookReply {
        event,
        message: message.clone(),
        context: message,
        block: None,
        outcomes,
    }))
}

fn silent(event: &'static str) -> Output {
    Output::Hook(HookReply {
        event,
        message: None,
        context: None,
        block: None,
        outcomes: Vec::new(),
    })
}

fn refused(event: &'static str, reason: &str) -> Output {
    let text = format!("scribe: not recorded — {reason}");
    Output::Hook(HookReply {
        event,
        message: Some(text.clone()),
        context: Some(text),
        block: None,
        outcomes: vec![HookOutcome {
            target: None,
            error_code: Some("provenance_failed"),
        }],
    })
}
