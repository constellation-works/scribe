//! `scribe hook stop` (ORB-27707 step 3b): check the transcript, fill in cite message ids
//! (§4.1), hand the turn's `#scribe` markers to the agent, and mark a
//! switched-on session's pass due (§5.2).
//!
//! The `#scribe` markers the user typed are handed to the agent
//! ([`super::annotation`], §14): each is echoed to the user and, with
//! its line and the `entry add` command that records it, given to the agent
//! (`additionalContext`). This hook calls no model. A transcript that fails the
//! provenance check is echoed once and stops the hook: nothing is filled,
//! cataloged or marked due.
//!
//! `UserPromptSubmit` runs before Claude Code writes the prompt to the
//! transcript, so an entry's cite starts with `message: null` and the hash
//! of the prompt. This hook finds the user message whose text has that hash
//! and records its uuid. When several messages match (the same prompt sent
//! twice), the one written closest to the cite's time is chosen.
//!
//! An open group member's transcript progress can advance independently of
//! cite filling or marker cataloging.
//!
//! Then the cadence: for a session that is on and not already due, the pass
//! is due once `[catalog] every_minutes` have passed since the last pass
//! (the record's `updated`, else when the session was switched on) or
//! `every_turns` of the user's turns have arrived since the record's
//! `scribed_through`. Minutes are checked first, so the transcript is read
//! only to count turns for cadence. Markers are cataloged also for a session
//! that is off or was never switched on.

use super::annotation::{self, AnnotationInput};
use super::reply::{HookReply, PROVENANCE_FAILED};
use crate::apps::entry;
use crate::apps::group;
use crate::apps::group::HookError;
use crate::apps::tag::ConfigReads;
use crate::control::{self, Since};
use crate::runtime::audit::HookOutcome;
use crate::runtime::resolve::refuse_forgotten;
use crate::runtime::{Ctx, PortError, TranscriptState};
use crate::transcript::{self, Message};
use crate::{Cite, Label, Provider, SessionId, Speaker, pass};
use std::path::Path;

/// What `hook stop` runs on: its parsed payload.
pub struct StopInput<'i> {
    /// The session.
    pub session: &'i SessionId,
    /// Its transcript.
    pub transcript_path: &'i Path,
    /// The cwd the payload named, for a line that carries none.
    pub cwd: Option<String>,
    /// The provider whose hook this is.
    pub provider: Provider,
    /// This host, as records name it.
    pub host: Label,
}

/// A user message with the hashes its text can match: the whole text, and
/// each text block on its own.
struct Hashed {
    message: Message,
    hashes: Vec<String>,
}

impl Hashed {
    fn new(message: Message) -> Self {
        let hashes = crate::capture::prompt_hashes(&message);
        Self { message, hashes }
    }
}

/// Run the hook on its parsed payload.
///
/// # Errors
///
/// [`HookError`]: a store error, an unreadable transcript, or a port.
pub fn run<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: StopInput<'_>,
) -> Result<HookReply, HookError<E>> {
    let StopInput {
        session,
        transcript_path,
        cwd,
        provider,
        host,
    } = input;
    let session = session.clone();
    let mut reply = HookReply {
        event: "Stop",
        message: None,
        context: None,
        block: None,
        outcomes: Vec::new(),
    };
    if let Err(error) = refuse_forgotten(ctx.store, &session) {
        if error.code() == "session_forgotten" {
            return Ok(reply);
        }
        return Err(error.into());
    }
    // Codex has its own host-attributed ingestion; its transcript is not
    // checked here.
    if provider != Provider::Codex
        && let Err(reason) = ctx.transcripts.check(provider, transcript_path, &session)
    {
        reply.message = Some(format!(
            "scribe: could not check for #scribe markers — {reason}"
        ));
        reply.outcomes.push(HookOutcome {
            target: Some(format!("session:{session}")),
            error_code: Some(PROVENANCE_FAILED),
        });
        return Ok(reply);
    }
    let mut outcomes: Vec<HookOutcome> = if provider == Provider::ClaudeCode {
        group::claude_turns(ctx, provider, &session, transcript_path)?
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    outcomes.extend(fill_cites(ctx, provider, transcript_path, &session)?);
    if !outcomes
        .iter()
        .any(|o| o.error_code == Some(PROVENANCE_FAILED))
    {
        let cataloged = annotation::run(
            ctx,
            AnnotationInput {
                session: &session,
                transcript_path,
                cwd,
                provider,
                host,
            },
        )?;
        if !cataloged.echoes.is_empty() {
            reply.message = Some(cataloged.echoes.join("\n"));
        }
        // Each handed-over marker reaches the agent (Claude Code delivers a
        // Stop's `additionalContext` to the model), which records it.
        if !cataloged.context.is_empty() {
            reply.context = Some(cataloged.context.join("\n"));
        }
        outcomes.extend(cataloged.outcomes);
    }
    if !outcomes
        .iter()
        .any(|o| o.error_code == Some(PROVENANCE_FAILED))
    {
        outcomes.extend(mark_due(ctx, provider, transcript_path, &session)?);
    }
    reply.outcomes = outcomes;
    Ok(reply)
}

/// Mark the session's pass due when the cadence says so; one outcome when
/// it did.
fn mark_due<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    transcript_path: &Path,
    session: &SessionId,
) -> Result<Vec<HookOutcome>, HookError<E>> {
    let store = ctx.store;
    let state = store.read().sessions().control()?;
    let Some(switch) = state
        .get(session)
        .filter(|_| control::needs_cadence(&state, session))
    else {
        return Ok(Vec::new());
    };
    let cadence = store.read().tags().config()?.catalog;
    let now = ctx.clock.now();
    let record = match store.read().sessions().get(session) {
        Ok(view) => Some(view.record),
        Err(crate::Error::SessionNotFound { .. }) => None,
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
        match ctx.transcripts.check(provider, transcript_path, session) {
            Ok(TranscriptState::Present) => {}
            Ok(TranscriptState::Missing) => return Ok(Vec::new()),
            Err(reason) => {
                tracing::warn!(%session, %reason, "not counting turns: the transcript check failed");
                return Ok(vec![HookOutcome {
                    target: Some(format!("session:{session}")),
                    error_code: Some(PROVENANCE_FAILED),
                }]);
            }
        }
        let bytes = read_transcript(ctx, provider, transcript_path)?;
        let conversation = pass::conversation(&transcript::parse(&bytes));
        since.turns = pass::user_turns_after(
            &conversation,
            record.as_ref().and_then(|r| r.scribed_through.as_deref()),
        );
        if !cadence.pass_due(switch, last_pass, now, since.turns) {
            return Ok(Vec::new());
        }
    }
    let marked = store.write(|tx| {
        tx.sessions()
            .change_control(|state| control::mark_due(state, session, &cadence, since, now))
    })?;
    Ok(if marked {
        vec![HookOutcome {
            target: Some(format!("session:{session}")),
            error_code: None,
        }]
    } else {
        Vec::new()
    })
}

fn read_transcript<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    transcript_path: &Path,
) -> Result<Vec<u8>, HookError<E>> {
    ctx.transcripts
        .read(provider, transcript_path)
        .map_err(|source| HookError::Read {
            what: transcript_path.display().to_string(),
            source,
        })
}

/// Fill the session's waiting cites; one outcome per entry filled, or one
/// refusal when the transcript fails the provenance check.
fn fill_cites<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    transcript_path: &Path,
    session: &SessionId,
) -> Result<Vec<HookOutcome>, HookError<E>> {
    let session = session.clone();
    if !entry::has_unfilled_cites(ctx.store, &session)? {
        return Ok(Vec::new());
    }
    match ctx.transcripts.check(provider, transcript_path, &session) {
        Ok(TranscriptState::Present) => {}
        Ok(TranscriptState::Missing) => return Ok(Vec::new()),
        Err(reason) => {
            tracing::warn!(%session, %reason, "not filling cites: the transcript check failed");
            return Ok(vec![HookOutcome {
                target: Some(format!("session:{session}")),
                error_code: Some(PROVENANCE_FAILED),
            }]);
        }
    }
    let bytes = read_transcript(ctx, provider, transcript_path)?;
    let prompts: Vec<Hashed> = transcript::parse(&bytes)
        .into_iter()
        .filter(|m| m.role == Speaker::User)
        .map(Hashed::new)
        .collect();
    let filled = entry::fill_cite_messages(ctx.store, &session, ctx.clock.now(), |cite| {
        best_match(&prompts, cite)
    })?;
    Ok(filled
        .into_iter()
        .map(|id| HookOutcome {
            target: Some(id.to_string()),
            error_code: None,
        })
        .collect())
}

/// The uuid of the matching message written closest to the cite's time.
fn best_match(prompts: &[Hashed], cite: &Cite) -> Option<String> {
    prompts
        .iter()
        .filter(|p| p.hashes.contains(&cite.prompt_sha256))
        .min_by_key(|p| (p.message.at - cite.at).abs())
        .map(|p| p.message.uuid.clone())
}
