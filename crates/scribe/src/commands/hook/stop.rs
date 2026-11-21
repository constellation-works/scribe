//! `scribe hook stop`: recover missed entry instructions, fill in cite
//! message ids (§4.1), and mark a switched-on session's pass due (§5.2).
//! Recovery echoes additions and refusals; it never calls a model.
//!
//! `UserPromptSubmit` runs before Claude Code writes the prompt to the
//! transcript, so an entry's cite starts with `message: null` and the hash
//! of the prompt. This hook finds the user message whose text has that hash
//! and records its uuid. When several messages match (the same prompt sent
//! twice), the one written closest to the cite's time is chosen.
//!
//! Recovery reads the transcript after every reply. Existing cites suppress
//! already captured lines, including prompt-hook captures and Stop replays.
//! With no missed instructions or waiting cites, it never locks or writes.
//!
//! Then the cadence: for a session that is on and not already due, the pass
//! is due once `[catalog] every_minutes` have passed since the last pass
//! (the record's `updated`, else when the session was switched on) or
//! `every_turns` of Daniel's turns have arrived since the record's
//! `scribed_through`. Minutes are checked first, so the transcript is read
//! only to count turns for cadence. Capture recovery also runs for a session
//! that is off or was never switched on.

use super::payload::{self, StopPayload, Transcript};
use crate::app::Context;
use crate::error::CliError;
use crate::output::{HookOutcome, Output};
use scribe_core::control::{self, Since};
use scribe_core::transcript::{self, Message};
use scribe_core::{Cite, SessionId, Speaker, pass};

/// The code of a payload whose transcript failed the provenance check.
const PROVENANCE_FAILED: &str = "provenance_failed";

/// Run the hook on its stdin payload.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let payload: StopPayload = payload::decode(input)?;
    let session = payload::session(&payload.session_id)?;
    let mut reply = super::recovery::run(&payload, &session, ctx)?;
    let mut outcomes = std::mem::take(&mut reply.outcomes);
    if !outcomes
        .iter()
        .any(|o| o.error_code == Some(PROVENANCE_FAILED))
    {
        outcomes.extend(fill_cites(&payload, &session, ctx)?);
    }
    if !outcomes
        .iter()
        .any(|o| o.error_code == Some(PROVENANCE_FAILED))
    {
        outcomes.extend(mark_due(&payload, &session, ctx)?);
    }
    reply.outcomes = outcomes;
    Ok(Output::Hook(reply))
}

/// Mark the session's pass due when the cadence says so; one outcome when
/// it did.
fn mark_due(
    payload: &StopPayload,
    session: &SessionId,
    ctx: &Context,
) -> Result<Vec<HookOutcome>, CliError> {
    let store = ctx.store();
    let state = store.control()?;
    let Some(switch) = state
        .get(session)
        .filter(|_| control::needs_cadence(&state, session))
    else {
        return Ok(Vec::new());
    };
    let cadence = store.config()?.catalog;
    let now = ctx.now();
    let record = match store.session(session) {
        Ok(view) => Some(view.record),
        Err(scribe_core::Error::SessionNotFound { .. }) => None,
        Err(err) => {
            tracing::warn!(%session, %err, "cannot read the session record; timing from /scribe on");
            None
        }
    };
    let last_pass = record
        .as_ref()
        .map(|r| r.updated)
        .or(switch.since)
        .unwrap_or(now);
    let mut since = Since {
        last_pass,
        turns: 0,
    };
    if !cadence.pass_due(switch, last_pass, now, 0) {
        match payload::check(ctx, &payload.transcript_path, session) {
            Ok(Transcript::Present) => {}
            Ok(Transcript::Missing) => return Ok(Vec::new()),
            Err(reason) => {
                tracing::warn!(%session, %reason, "not counting turns: the transcript check failed");
                return Ok(vec![HookOutcome {
                    target: Some(format!("session:{session}")),
                    error_code: Some(PROVENANCE_FAILED),
                    issue: None,
                }]);
            }
        }
        let bytes = read_transcript(ctx, payload)?;
        let conversation = pass::conversation(&transcript::parse(&bytes));
        since.turns = pass::user_turns_after(
            &conversation,
            record.as_ref().and_then(|r| r.scribed_through.as_deref()),
        );
        if !cadence.pass_due(switch, last_pass, now, since.turns) {
            return Ok(Vec::new());
        }
    }
    let marked =
        store.update_control(|state| control::mark_due(state, session, &cadence, since, now))?;
    Ok(if marked {
        vec![HookOutcome {
            target: Some(format!("session:{session}")),
            error_code: None,
            issue: None,
        }]
    } else {
        Vec::new()
    })
}

fn read_transcript(ctx: &Context, payload: &StopPayload) -> Result<Vec<u8>, CliError> {
    payload::read_for(ctx, &payload.transcript_path).map_err(|source| CliError::Read {
        what: payload.transcript_path.display().to_string(),
        source,
    })
}

/// Fill the session's waiting cites; one outcome per entry filled, or one
/// refusal when the transcript fails the provenance check.
fn fill_cites(
    payload: &StopPayload,
    session: &SessionId,
    ctx: &Context,
) -> Result<Vec<HookOutcome>, CliError> {
    let session = session.clone();
    if !ctx.store().has_unfilled_cites(&session)? {
        return Ok(Vec::new());
    }
    match payload::check(ctx, &payload.transcript_path, &session) {
        Ok(Transcript::Present) => {}
        Ok(Transcript::Missing) => return Ok(Vec::new()),
        Err(reason) => {
            tracing::warn!(%session, %reason, "not filling cites: the transcript check failed");
            return Ok(vec![HookOutcome {
                target: Some(format!("session:{session}")),
                error_code: Some(PROVENANCE_FAILED),
                issue: None,
            }]);
        }
    }
    let bytes = read_transcript(ctx, payload)?;
    let prompts: Vec<Hashed> = transcript::parse(&bytes)
        .into_iter()
        .filter(|m| m.role == Speaker::User)
        .map(Hashed::new)
        .collect();
    let filled = ctx
        .store()
        .fill_cite_messages(&session, ctx.now(), |cite| best_match(&prompts, cite))?;
    Ok(filled
        .into_iter()
        .map(|id| HookOutcome {
            target: Some(id.to_string()),
            error_code: None,
            issue: None,
        })
        .collect())
}

/// A user message with the hashes its text can match: the whole text, and
/// each text block on its own.
struct Hashed {
    message: Message,
    hashes: Vec<String>,
}

impl Hashed {
    fn new(message: Message) -> Self {
        let hashes = scribe_core::capture::prompt_hashes(&message);
        Self { message, hashes }
    }
}

/// The uuid of the matching message written closest to the cite's time.
fn best_match(prompts: &[Hashed], cite: &Cite) -> Option<String> {
    prompts
        .iter()
        .filter(|p| p.hashes.contains(&cite.prompt_sha256))
        .min_by_key(|p| (p.message.at - cite.at).abs())
        .map(|p| p.message.uuid.clone())
}
