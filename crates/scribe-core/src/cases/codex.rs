//! Codex lifecycle adapter (ORB-27707 step 3c). Only host-attributed
//! user.text messages can drive writes. A before-persistence prompt is
//! deferred to Stop, never upgraded from an unverified hook payload. Cursors
//! contain ids only, not private prompts.
//!
//! A `#scribe` marker inside a sentence (2_design.md §14) is echoed at submit
//! once the typed turn is verified in the rollout, and cataloged by Stop from
//! the rollout (`annotation.rs`), as under Claude Code.
//!
//! The CLI decodes the payload, refuses a mismatched `hook_event_name` and
//! runs `SessionStart` itself; [`run`] is the rest of the ingest. The
//! `SessionEnd` and `Stop` payloads are decoded up front, and a decode error
//! is raised where the old adapter decoded them.

use crate::capture::OriginClaims;
use crate::cases::audit::PersistedAudit;
use crate::cases::hook::{
    HookError, HookOutcome, HookPortError, HookReply, PROVENANCE_FAILED, command_text, is_control,
};
use crate::cases::stop::{self, StopInput};
use crate::cases::user_prompt_submit::{self, PromptInput};
use crate::cases::{Ctx, TranscriptState, group_turns, session_end};
use crate::codex::TypedTurn;
use crate::message::NewMessage;
use crate::{Config, Label, Models, Provider, SessionId, annotation, capture, control};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The fields of a `Stop` payload the Stop hook runs on.
pub struct CodexStop {
    /// The session.
    pub session: SessionId,
    /// Its transcript.
    pub transcript_path: PathBuf,
    /// The cwd the payload named.
    pub cwd: Option<String>,
}

/// What the Codex adapter runs on.
pub struct CodexInput<'i, E, F, R> {
    /// The hook event (`UserPromptSubmit`, `Stop` or `SessionEnd`).
    pub event: &'static str,
    /// The decoded payload.
    pub payload: &'i Value,
    /// The provider whose hook this is.
    pub provider: Provider,
    /// This host, as records name it.
    pub host: Label,
    /// The `SessionEnd` payload's session, decoded up front for that event.
    pub session_end: Option<Result<SessionId, E>>,
    /// The `Stop` payload, decoded up front for that event.
    pub stop: Option<Result<CodexStop, E>>,
    /// Builds the `Models` adapter for Stop's config.
    pub models: F,
    /// Reads a named session's past transcript for `/scribe group add`.
    pub read_past: R,
}

/// Whether `prompt` holds a `#scribe` annotation marker, cataloged at Stop.
#[must_use]
pub fn has_marks(prompt: &str) -> bool {
    !is_control(prompt) && !annotation::marks(prompt).is_empty()
}

fn relevant(turn: &TypedTurn) -> bool {
    let text = command_text(&turn.message.text());
    control::parse(&text).is_some() || !capture::lines(&text).is_empty()
}

/// A refusal: nothing recorded, `reason` told to the user and the agent.
#[must_use]
pub fn refused(event: &'static str, reason: &str) -> HookReply {
    let text = format!("scribe: not recorded — {reason}");
    HookReply {
        event,
        message: Some(text.clone()),
        context: Some(text),
        block: None,
        outcomes: vec![HookOutcome {
            target: None,
            error_code: Some(PROVENANCE_FAILED),
        }],
    }
}

fn silent(event: &'static str) -> HookReply {
    HookReply {
        event,
        message: None,
        context: None,
        block: None,
        outcomes: Vec::new(),
    }
}

fn prepend_outcomes(out: &mut HookReply, mut prefix: Vec<HookOutcome>) {
    if prefix.is_empty() {
        return;
    }
    prefix.append(&mut out.outcomes);
    out.outcomes = prefix;
}

/// The pre-decoded payload of `event`, raised here as the old adapter raised
/// its decode error.
fn decoded<T, E>(pre: &mut Option<Result<T, E>>, event: &str) -> Result<T, HookError<E>> {
    match pre.take() {
        Some(result) => result.map_err(HookError::Port),
        None => Err(HookError::Usage(format!("no {event} payload was decoded"))),
    }
}

/// Run Stop on its pre-decoded payload.
fn run_stop<E: HookPortError, M: Models>(
    ctx: &Ctx<'_, E>,
    pre: &mut Option<Result<CodexStop, E>>,
    provider: Provider,
    host: &Label,
    models: &mut Option<impl FnOnce(&Config) -> M>,
) -> Result<HookReply, HookError<E>> {
    let payload = decoded(pre, "Stop")?;
    let models = models
        .take()
        .ok_or_else(|| HookError::Usage("Stop ran twice in one Codex hook".into()))?;
    stop::run(
        ctx,
        StopInput {
            session: &payload.session,
            transcript_path: &payload.transcript_path,
            cwd: payload.cwd,
            provider,
            host: host.clone(),
            models,
        },
    )
}

/// Ingest a Codex lifecycle hook other than `SessionStart`.
///
/// # Errors
///
/// [`HookError`]: a store error, an unreadable transcript, a payload decode
/// error raised by the port, or a port.
pub fn run<E, M, F, R>(
    ctx: &Ctx<'_, E>,
    input: CodexInput<'_, E, F, R>,
) -> Result<HookReply, HookError<E>>
where
    E: HookPortError,
    M: Models,
    F: FnOnce(&Config) -> M,
    R: FnMut(Provider, &SessionId, &Path) -> Result<Option<Vec<NewMessage>>, E>,
{
    let CodexInput {
        event,
        payload: value,
        provider,
        host,
        session_end: mut end_payload,
        stop: mut stop_payload,
        models,
        mut read_past,
    } = input;
    let mut models = Some(models);
    let Some(raw_id) = value["session_id"].as_str() else {
        return Ok(refused(
            event,
            "Codex hook requires session_id; nothing changed",
        ));
    };
    let Ok(id) = raw_id.parse::<SessionId>() else {
        return Ok(refused(event, "invalid Codex session_id; nothing changed"));
    };
    let Some(path) = value["transcript_path"].as_str().map(Path::new) else {
        return Ok(refused(
            event,
            "Codex supplied no transcript; recording is unavailable. Use a supported desktop user thread and review scribe hooks",
        ));
    };
    match ctx.transcripts.check(provider, path, &id) {
        Ok(TranscriptState::Present) => {}
        Ok(TranscriptState::Missing) => {
            return Ok(refused(
                event,
                "waiting for Codex transcript persistence; nothing recorded yet; Stop will retry verified inputs",
            ));
        }
        Err(reason) => return Ok(refused(event, &reason)),
    }
    let bytes = ctx
        .transcripts
        .read(provider, path)
        .map_err(|source| HookError::Read {
            what: path.display().to_string(),
            source,
        })?;
    let rollout = match crate::codex::parse(&bytes) {
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
        .store
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
        let session = decoded(&mut end_payload, event)?;
        return session_end::run(ctx, &session);
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
    let mut persisted =
        (event == "Stop").then(|| PersistedAudit::new(ctx.store, "hook stop", ctx.clock.now()));
    let group_outcomes: Vec<HookOutcome> = group_turns::codex(ctx, provider, &id, &rollout.typed)?
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
    // Ends without a replay: Stop runs (or the reply is silent), after the
    // group turns' outcomes, and the persisted audit is disarmed.
    let finish = |group_outcomes: Vec<HookOutcome>,
                  persisted: Option<&mut PersistedAudit<'_>>,
                  stop_payload: &mut Option<Result<CodexStop, E>>,
                  models: &mut Option<F>|
     -> Result<HookReply, HookError<E>> {
        let mut out = if event == "Stop" {
            run_stop(ctx, stop_payload, provider, &host, models)?
        } else {
            silent(event)
        };
        prepend_outcomes(&mut out, group_outcomes);
        if let Some(persisted) = persisted {
            persisted.disarm();
        }
        Ok(out)
    };
    let candidates: Vec<_> = rollout.typed.iter().filter(|t| relevant(t)).collect();
    if candidates.is_empty() {
        return finish(
            group_outcomes,
            persisted.as_mut(),
            &mut stop_payload,
            &mut models,
        );
    }
    let before = ctx.store.read().sessions().control()?;
    if before
        .codex_cursors
        .get(&id)
        .is_some_and(|cursor| candidates.last().is_some_and(|t| &t.message.uuid == cursor))
    {
        return finish(
            group_outcomes,
            persisted.as_mut(),
            &mut stop_payload,
            &mut models,
        );
    }
    let _ingestion = ctx.store.lock_codex_ingestion(&id)?;
    let state = ctx.store.read().sessions().control()?;
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
        // The replayed turn is the prompt hook's payload with `source:
        // "typed"`, its only origin claim.
        let text = command_text(&turn.message.text());
        let mut origin = OriginClaims::default();
        origin.push_claim(Some(&Value::from("typed")));
        let reply = user_prompt_submit::run(
            ctx,
            &PromptInput {
                session: &id,
                transcript_path: path,
                cwd: &rollout.cwd,
                prompt: &text,
                agent_originated: origin.is_agent_originated(&text),
                provider,
                host: host.clone(),
            },
            &mut read_past,
        )?;
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
        ctx.store.write(|tx| {
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
    // Stop's own echoes (the markers it cataloged) follow the replayed
    // turns' replies, so a catch-up Stop still reports what it recorded.
    let mut agent_only = None;
    if event == "Stop" {
        let reply = run_stop(ctx, &mut stop_payload, provider, &host, &mut models)?;
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
    Ok(HookReply {
        event,
        message,
        context,
        block: None,
        outcomes,
    })
}
