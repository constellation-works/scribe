//! Codex lifecycle adapter. Only host-attributed user.text messages can drive
//! writes. A before-persistence prompt is deferred to Stop, never upgraded from
//! an unverified hook payload. Cursors contain ids only, not private prompts.
//!
//! A `#scribe` marker inside a sentence (2_design.md §14) is echoed at submit
//! once the typed turn is verified in the rollout, and cataloged by Stop from
//! the rollout (`annotation.rs`), as under Claude Code.

use super::{payload, session_end, session_start, stop, user_prompt_submit};
use crate::{
    app::Context,
    error::CliError,
    output::{HookOutcome, HookReply, Output},
};
use scribe_core::{Provider, annotation, capture, codex::TypedTurn, control};
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
                    | control::Switch::Catalog(_)
                    | control::Switch::Group(_)
                    | control::Switch::GroupAdd { .. }
                    | control::Switch::GroupMove { .. }
                    | control::Switch::GroupRemove(_)
                    | control::Switch::SessionUpdate { .. }
            )
        ) {
            return candidate;
        }
    }
    original.to_owned()
}

/// Whether a typed Codex input is a session control, `/scribe …` or its
/// `$scribe …` skill form.
pub(super) fn is_control(text: &str) -> bool {
    control::parse(&command_text(text)).is_some()
}

/// Whether `prompt` holds a `#scribe` annotation marker, cataloged at Stop.
fn has_marks(prompt: &str) -> bool {
    !is_control(prompt) && !annotation::marks(prompt).is_empty()
}

fn relevant(turn: &TypedTurn) -> bool {
    let text = command_text(&turn.message.text());
    control::parse(&text).is_some() || !capture::lines(&text).is_empty()
}

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
        .read()
        .sessions()
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
            if control::parse(&command_text(prompt)).is_none()
                && capture::lines(prompt).is_empty()
                && !has_marks(prompt)
            {
                return Ok(silent(event));
            }
            return Ok(refused(
                event,
                "typed input not yet verified in the host transcript; nothing recorded yet; Stop will retry. Agent or skill-injected text cannot activate scribe",
            ));
        }
    }
    let mut persisted = (event == "Stop")
        .then(|| crate::commands::PersistedAudit::new(ctx.store(), "hook stop", ctx.now()));
    let group_outcomes: Vec<HookOutcome> = super::group_turns::codex(ctx, &id, &rollout.typed)?
        .into_iter()
        .collect();
    if let Some(persisted) = persisted.as_mut() {
        for outcome in &group_outcomes {
            if outcome.error_code.is_none()
                && let Some(target) = &outcome.target
            {
                persisted.record(target.clone());
            }
        }
    }
    let candidates: Vec<_> = rollout.typed.iter().filter(|t| relevant(t)).collect();
    if candidates.is_empty() {
        return finish_without_replay(event, input, ctx, group_outcomes, persisted.as_mut());
    }
    let before = ctx.store().read().sessions().control()?;
    if before
        .codex_cursors
        .get(&id)
        .is_some_and(|cursor| candidates.last().is_some_and(|t| &t.message.uuid == cursor))
    {
        return finish_without_replay(event, input, ctx, group_outcomes, persisted.as_mut());
    }
    let _ingestion = ctx.store().lock_codex_ingestion(&id)?;
    let state = ctx.store().read().sessions().control()?;
    let cursor = state.codex_cursors.get(&id);
    let start = match cursor {
        Some(cursor) => match candidates.iter().position(|t| &t.message.uuid == cursor) {
            Some(n) => n + 1,
            None => {
                let mut out = refused(
                    event,
                    "Codex ingestion cursor disappeared; transcript was rewritten. Nothing replayed; inspect the session before resuming",
                );
                prepend_outcomes(&mut out, group_outcomes);
                if let Some(persisted) = persisted.as_mut() {
                    persisted.disarm();
                }
                return Ok(out);
            }
        },
        None => candidates
            .iter()
            .position(|t| t.turn_id == turn_id)
            .unwrap_or(candidates.len()),
    };
    let mut replies = Vec::new();
    let mut outcomes = group_outcomes;
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
            ctx.store().write(|tx| {
                tx.sessions().change_control(|state| {
                    if let Some(switch) = state.sessions.get_mut(&id) {
                        switch.provider = Provider::Codex;
                    }
                    state
                        .codex_cursors
                        .insert(id.clone(), turn.message.uuid.clone());
                })
            })?;
            outcomes.push(HookOutcome {
                target: Some(format!("session:{id}")),
                error_code: None,
            });
        }
    }
    // Stop's own echoes (the markers it cataloged) follow the replayed
    // turns' replies, so a catch-up Stop still reports what it recorded.
    let mut agent_only = None;
    if event == "Stop"
        && let Output::Hook(reply) = stop::run(input, ctx)?
    {
        replies.extend(reply.message);
        agent_only = reply.context;
        outcomes.extend(reply.outcomes);
    }
    if let Some(persisted) = persisted.as_mut() {
        persisted.disarm();
    }
    let message = (!replies.is_empty()).then(|| replies.join("\n"));
    // Codex Stop permits additionalContext JSON; no plain stdout, and no
    // blocking decision that would accidentally ask the model to continue.
    // The agent gets the echoes, then a given-up marker's line and note.
    let context = [message.clone(), agent_only]
        .into_iter()
        .flatten()
        .reduce(|a, b| format!("{a}\n{b}"));
    Ok(Output::Hook(HookReply {
        event,
        message: message.clone(),
        context,
        block: None,
        outcomes,
    }))
}

fn finish_without_replay(
    event: &'static str,
    input: &[u8],
    ctx: &Context,
    group_outcomes: Vec<HookOutcome>,
    persisted: Option<&mut crate::commands::PersistedAudit<'_>>,
) -> Result<Output, CliError> {
    let mut out = if event == "Stop" {
        stop::run(input, ctx)?
    } else {
        silent(event)
    };
    prepend_outcomes(&mut out, group_outcomes);
    if let Some(persisted) = persisted {
        persisted.disarm();
    }
    Ok(out)
}

fn prepend_outcomes(out: &mut Output, mut prefix: Vec<HookOutcome>) {
    if prefix.is_empty() {
        return;
    }
    if let Output::Hook(reply) = out {
        prefix.append(&mut reply.outcomes);
        reply.outcomes = prefix;
    }
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
