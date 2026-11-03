//! `scribe hook stop`: after a reply, fill in the cite's message uuid of the
//! entries this session recorded (docs/design/scribe/2_design.md §4.1), and
//! mark a switched-on session's pass due (§5.2). It never calls a model.
//!
//! `UserPromptSubmit` runs before Claude Code writes the prompt to the
//! transcript, so an entry's cite starts with `message: null` and the hash
//! of the prompt. This hook finds the user message whose text has that hash
//! and records its uuid. When several messages match (the same prompt sent
//! twice), the one written closest to the cite's time is chosen.
//!
//! It runs after every reply, so the common case is cheap: when no entry of
//! the session is waiting, it reads the entries, finds none, and goes on
//! without opening the transcript, locking, writing or auditing.
//!
//! Then the cadence: for a session that is on and not already due, the pass
//! is due once `[catalog] every_minutes` have passed since the last pass
//! (the record's `updated`, else when the session was switched on) or
//! `every_turns` of Daniel's turns have arrived since the record's
//! `scribed_through`. Minutes are checked first, so the transcript is read
//! only to count turns. A session that is off or was never switched on
//! costs one read of `control.json`. It prints nothing.

use super::payload::{self, StopPayload, Transcript};
use crate::app::Context;
use crate::error::CliError;
use crate::output::{HookOutcome, HookReply, Output};
use scribe_core::capture::prompt_sha256;
use scribe_core::control::{self, Since};
use scribe_core::transcript::{self, Message};
use scribe_core::{Cite, SessionId, Speaker, pass};

/// The hook event this runs for.
const EVENT: &str = "Stop";
/// The code of a payload whose transcript failed the provenance check.
const PROVENANCE_FAILED: &str = "provenance_failed";

/// Run the hook on its stdin payload.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let payload: StopPayload = payload::decode(input)?;
    let session = payload::session(&payload.session_id)?;
    let mut outcomes = fill_cites(&payload, &session, ctx)?;
    if !outcomes
        .iter()
        .any(|o| o.error_code == Some(PROVENANCE_FAILED))
    {
        outcomes.extend(mark_due(&payload, &session, ctx)?);
    }
    Ok(silent(outcomes))
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
        match payload::check_transcript(&payload.transcript_path, session, ctx.transcripts()) {
            Ok(Transcript::Present) => {}
            Ok(Transcript::Missing) => return Ok(Vec::new()),
            Err(reason) => {
                tracing::warn!(%session, %reason, "not counting turns: the transcript check failed");
                return Ok(vec![HookOutcome {
                    target: Some(format!("session:{session}")),
                    error_code: Some(PROVENANCE_FAILED),
                }]);
            }
        }
        let bytes = read_transcript(payload)?;
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
        }]
    } else {
        Vec::new()
    })
}

fn read_transcript(payload: &StopPayload) -> Result<Vec<u8>, CliError> {
    std::fs::read(&payload.transcript_path).map_err(|source| CliError::Read {
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
    match payload::check_transcript(&payload.transcript_path, &session, ctx.transcripts()) {
        Ok(Transcript::Present) => {}
        Ok(Transcript::Missing) => return Ok(Vec::new()),
        Err(reason) => {
            tracing::warn!(%session, %reason, "not filling cites: the transcript check failed");
            return Ok(vec![HookOutcome {
                target: Some(format!("session:{session}")),
                error_code: Some(PROVENANCE_FAILED),
            }]);
        }
    }
    let bytes = read_transcript(payload)?;
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
        let mut hashes = vec![prompt_sha256(&message.text())];
        hashes.extend(message.blocks.iter().map(|b| prompt_sha256(b)));
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

fn silent(outcomes: Vec<HookOutcome>) -> Output {
    Output::Hook(HookReply {
        event: EVENT,
        message: None,
        context: None,
        block: None,
        outcomes,
    })
}
