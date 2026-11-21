//! Recover entry instructions whose prompt hook was missed. Existing cites
//! are the progress record: rescanning also retries an interrupted capture,
//! without a cursor that could advance past an unwritten instruction.

use super::payload::{self, PromptPayload, StopPayload, Transcript};
use super::user_prompt_submit::{self, Recovery};
use crate::app::Context;
use crate::error::CliError;
use crate::output::HookReply;
use scribe_core::capture;
use scribe_core::transcript;
use scribe_core::{Provider, SessionId, Speaker};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn run(
    payload: &StopPayload,
    session: &SessionId,
    ctx: &Context,
) -> Result<HookReply, CliError> {
    let mut reply = HookReply {
        event: "Stop",
        message: None,
        context: None,
        block: None,
        outcomes: Vec::new(),
    };
    // Codex has its own host-attributed ingestion and cursor. Never recover
    // its broader conversation view as if every user-role turn were typed.
    if ctx.provider() == Provider::Codex {
        return Ok(reply);
    }
    match payload::check(ctx, &payload.transcript_path, session) {
        Ok(Transcript::Present) => {}
        Ok(Transcript::Missing) => return Ok(reply),
        Err(reason) => {
            reply.message = Some(format!(
                "scribe: could not check for missed #scribe lines — {reason}"
            ));
            reply.outcomes.push(crate::output::HookOutcome {
                target: Some(format!("session:{session}")),
                error_code: Some("provenance_failed"),
                issue: None,
            });
            return Ok(reply);
        }
    }
    let bytes =
        payload::read_for(ctx, &payload.transcript_path).map_err(|source| CliError::Read {
            what: payload.transcript_path.display().to_string(),
            source,
        })?;
    let messages = transcript::parse(&bytes);
    // Keep the original origin fields and per-message cwd. Parsing the
    // conversation alone deliberately strips those provider-specific claims.
    let candidates: BTreeSet<&str> = messages
        .iter()
        .filter(|m| m.role == Speaker::User && !capture::lines(&m.text()).is_empty())
        .map(|m| m.uuid.as_str())
        .collect();
    if candidates.is_empty() {
        return Ok(reply);
    }
    #[derive(serde::Deserialize)]
    struct Identity {
        uuid: Option<String>,
    }
    let mut raw = BTreeMap::new();
    for line in bytes.split(|&b| b == b'\n') {
        // Most lines are tool payloads. Keep a DOM only for candidate user
        // messages, so an old instruction does not retain the whole file.
        if let Ok(Identity { uuid: Some(uuid) }) = serde_json::from_slice(line)
            && candidates.contains(uuid.as_str())
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(line)
        {
            raw.entry(uuid).or_insert(value);
        }
    }
    let mut echoes = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        if message.role != Speaker::User || capture::lines(&message.text()).is_empty() {
            continue;
        }
        let text = message.text();
        let mut recorded: BTreeSet<u32> = ctx
            .store()
            .entries_from_prompt(session, &capture::prompt_sha256(&text))?
            .iter()
            .filter_map(|entry| entry.cite.as_ref().map(|cite| cite.line))
            .collect();
        // A prompt hook may have received one text block rather than the
        // joined message. Translate its line numbers into the joined text so
        // recovery cannot duplicate that block, including a partial capture.
        let mut offset = 0u32;
        if message.blocks.len() > 1 {
            for block in &message.blocks {
                for entry in ctx
                    .store()
                    .entries_from_prompt(session, &capture::prompt_sha256(block))?
                {
                    if let Some(cite) = entry.cite {
                        recorded.insert(offset.saturating_add(cite.line));
                    }
                }
                offset = offset
                    .saturating_add(u32::try_from(block.split('\n').count()).unwrap_or(u32::MAX))
                    .saturating_add(1);
            }
        }
        if capture::lines(&text)
            .iter()
            .all(|(line, _)| recorded.contains(line))
        {
            continue;
        }
        let Some(mut value) = raw.get(&message.uuid).cloned() else {
            continue;
        };
        value["session_id"] = serde_json::json!(session);
        value["transcript_path"] = serde_json::json!(payload.transcript_path);
        value["prompt"] = serde_json::json!(message.text());
        if value["cwd"].is_null() {
            value["cwd"] = serde_json::json!(payload.cwd.as_deref().unwrap_or_default());
        }
        // The empty placeholder lets the origin guard skip synthetic turns
        // even without cwd. It must never reach capture as a citation.
        let prompt: PromptPayload = match serde_json::from_value(value) {
            Ok(prompt) => prompt,
            Err(err) => {
                echoes.push(format!(
                    "scribe: could not recover #scribe lines in message {} — missing or invalid prompt metadata: {err}",
                    message.uuid
                ));
                reply.outcomes.push(crate::output::HookOutcome {
                    target: Some(format!("session:{session}")),
                    error_code: Some("hook_payload_invalid"),
                    issue: None,
                });
                continue;
            }
        };
        if payload::agent_originated(&prompt)
            || scribe_core::control::parse(&prompt.prompt).is_some()
        {
            continue;
        }
        if prompt.cwd.is_empty() {
            echoes.push(format!(
                "scribe: could not recover #scribe lines in message {} — neither the transcript nor the Stop payload supplies a cwd",
                message.uuid
            ));
            reply.outcomes.push(crate::output::HookOutcome {
                target: Some(format!("session:{session}")),
                error_code: Some("hook_payload_invalid"),
                issue: None,
            });
            continue;
        }
        // A covered line writes no entry, so it is never in `recorded`.
        // Asking the store, after cwd is known, keeps Stop from repeating
        // the prompt hook's "already covered" echo on every later turn,
        // and from capturing a line that was covered when it was typed
        // once that entry is retired or superseded. A missed prompt hook
        // stays quiet too. Config or redactor failure skips this
        // pre-check; capture still decides, and a covered echo is dropped
        // while recovering. Capture only sees entries active now, so this
        // pre-check is what keeps the earlier line unwritten.
        if let Ok(config) = ctx.config()
            && let Ok(redactor) = ctx.redactor(&config)
        {
            let turn = capture::Turn {
                session: session.clone(),
                at: message.at,
                prompt_sha256: capture::prompt_sha256(&prompt.prompt),
                cwd: prompt.cwd.clone(),
                project: super::project::of(std::path::Path::new(&prompt.cwd)),
                provider: scribe_core::Label::sanitized(
                    ctx.provider().as_str(),
                    ctx.provider().as_str(),
                ),
                host: ctx.host(),
            };
            if let Ok(covered) =
                ctx.store()
                    .covered_instruction_lines(&prompt.prompt, &turn, &redactor)
            {
                recorded.extend(covered);
            }
        }
        if capture::lines(&prompt.prompt)
            .iter()
            .all(|(line, _)| recorded.contains(line))
        {
            continue;
        }
        if let crate::output::Output::Hook(captured) = user_prompt_submit::recover(
            &prompt,
            ctx,
            Recovery {
                message,
                before: &messages[..index],
                recorded,
            },
        )? {
            echoes.extend(captured.message);
            reply.outcomes.extend(captured.outcomes);
        }
    }
    reply.message = (!echoes.is_empty()).then(|| echoes.join("\n"));
    Ok(reply)
}
