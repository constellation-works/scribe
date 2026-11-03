//! `scribe hook stop`: after a reply, fill in the cite's message uuid of the
//! entries this session recorded (docs/design/scribe/2_design.md §4.1).
//!
//! `UserPromptSubmit` runs before Claude Code writes the prompt to the
//! transcript, so an entry's cite starts with `message: null` and the hash
//! of the prompt. This hook finds the user message whose text has that hash
//! and records its uuid. When several messages match (the same prompt sent
//! twice), the one written closest to the cite's time is chosen.
//!
//! It runs after every reply, so the common case is cheap: when no entry of
//! the session is waiting, it reads the entries, finds none, and returns
//! without opening the transcript, locking, writing or auditing. It prints
//! nothing.

use super::payload::{self, StopPayload, Transcript};
use crate::app::Context;
use crate::error::CliError;
use crate::output::{HookOutcome, HookReply, Output};
use scribe_core::capture::prompt_sha256;
use scribe_core::transcript::{self, Message};
use scribe_core::{Cite, Speaker};

/// The hook event this runs for.
const EVENT: &str = "Stop";
/// The code of a payload whose transcript failed the provenance check.
const PROVENANCE_FAILED: &str = "provenance_failed";

/// Run the hook on its stdin payload.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let payload: StopPayload = payload::decode(input)?;
    let session = payload::session(&payload.session_id)?;
    if !ctx.store().has_unfilled_cites(&session)? {
        return Ok(silent(Vec::new()));
    }
    match payload::check_transcript(&payload.transcript_path, &session, ctx.transcripts()) {
        Ok(Transcript::Present) => {}
        Ok(Transcript::Missing) => return Ok(silent(Vec::new())),
        Err(reason) => {
            tracing::warn!(%session, %reason, "not filling cites: the transcript check failed");
            return Ok(silent(vec![HookOutcome {
                target: Some(format!("session:{session}")),
                error_code: Some(PROVENANCE_FAILED),
            }]));
        }
    }
    let bytes = std::fs::read(&payload.transcript_path).map_err(|source| CliError::Read {
        what: payload.transcript_path.display().to_string(),
        source,
    })?;
    let prompts: Vec<Hashed> = transcript::parse(&bytes)
        .into_iter()
        .filter(|m| m.role == Speaker::User)
        .map(Hashed::new)
        .collect();
    let filled = ctx
        .store()
        .fill_cite_messages(&session, ctx.now(), |cite| best_match(&prompts, cite))?;
    Ok(silent(
        filled
            .into_iter()
            .map(|id| HookOutcome {
                target: Some(id.to_string()),
                error_code: None,
            })
            .collect(),
    ))
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
        outcomes,
    })
}
