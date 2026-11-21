//! Catalog the `#scribe` markers Daniel typed inside his sentences, at the
//! Stop of the turn (docs/design/scribe/2_design.md §14).
//!
//! The prompt hook only echoed the marker. Here, after the reply, each
//! typed turn of the transcript with a marker that `marks.json` has not
//! settled gets one bounded `models.annotation` call ([`MAX_TURNS_PER_STOP`]
//! turns per Stop, oldest first). Each answer is checked without a model
//! (`annotation::check`) and recorded in one store call that writes the
//! entries and `marks.json` (`Store::record_marks`):
//!
//! - an entry: Daniel's, content scribe's statement, `quote` his words,
//!   `stated_by` scribe:annotation, no proposal;
//! - none: nothing written, never asked again;
//! - refused (a check, a cover, redaction, or a marker refused before any
//!   call): never asked again;
//! - a failed call: nothing written, the marker stays pending and the next
//!   Stop retries it, up to [`MAX_ATTEMPTS`] calls, then refused
//!   `model_failed`.
//!
//! Daniel sees an echo for every outcome except a marker the prompt hook
//! already refused, which is recorded and audited here silently. A turn
//! with no marker, or whose markers are all settled, reads only the
//! transcript and `marks.json`: no lock, no write, no call.
//!
//! A marker opening a line is a folded form 1–3 (§14.6): before store
//! format 22 such a line was recorded verbatim by the prompt hook, or by
//! Stop recovery, and its entry cites that prompt and line. A marker on a
//! line that already has an entry is left alone, so an older transcript
//! is not cataloged twice.
//!
//! Under Codex the Stop reads Codex's own rollout (`scribe_core::codex`):
//! its user messages are the host-attributed typed inputs, its cwd the
//! session's, and a `$scribe` skill control is skipped as a `/scribe` one is.

use super::payload::{self, PromptPayload, StopPayload, Transcript};
use super::project;
use super::user_prompt_submit::{chain, quote};
use crate::app::Context;
use crate::error::CliError;
use crate::output::HookOutcome;
use scribe_core::annotation::{self, Checked, MAX_ATTEMPTS, MAX_TURNS_PER_STOP, Mark};
use scribe_core::capture::{self, Turn};
use scribe_core::transcript::{self, Message};
use scribe_core::{
    COVERED, Config, Label, MODEL_FAILED, MarkChange, MarkKey, MarkResult, Models, NEEDS_REDACTION,
    Provider, SessionId, Speaker,
};
use std::collections::{BTreeMap, BTreeSet};

/// What the hook tells Daniel and audits.
#[derive(Default)]
pub(super) struct Cataloged {
    pub(super) echoes: Vec<String>,
    pub(super) outcomes: Vec<HookOutcome>,
}

/// One marked turn still to catalog.
struct Work<'a> {
    message: &'a Message,
    index: usize,
    turn: Turn,
    /// Markers refused before any call and not yet recorded.
    refused: Vec<Mark>,
    /// Markers the model reads.
    open: Vec<Mark>,
}

/// How one update's result is echoed.
enum Say {
    /// Recorded silently: the prompt hook already echoed it.
    Silent,
    /// Its reason, for a refusal the checks made.
    Reason(String),
    /// The call's error, for a failure.
    Failed(String),
    /// The model's answer.
    Answer,
}

impl Say {
    /// Its words, when it has any.
    fn text(self) -> Option<String> {
        match self {
            Self::Reason(text) | Self::Failed(text) => Some(text),
            Self::Silent | Self::Answer => None,
        }
    }
}

/// Catalog the session's unsettled markers, building the model adapter
/// with `models` only when a call is due.
pub(super) fn run<M: Models>(
    payload: &StopPayload,
    session: &SessionId,
    ctx: &Context,
    models: impl FnOnce(&Config) -> M,
) -> Result<Cataloged, CliError> {
    let mut out = Cataloged::default();
    // Stop checked the transcript first and reported a failure.
    if !matches!(
        payload::check(ctx, &payload.transcript_path, session),
        Ok(Transcript::Present)
    ) {
        return Ok(out);
    }
    let bytes =
        payload::read_for(ctx, &payload.transcript_path).map_err(|source| CliError::Read {
            what: payload.transcript_path.display().to_string(),
            source,
        })?;
    // Codex: the rollout's verified typed inputs and assistant text, and
    // the session's cwd; the dispatcher already checked its identity.
    let (messages, codex_cwd) = if ctx.provider() == Provider::Codex {
        match scribe_core::codex::parse(&bytes) {
            Ok(rollout) if rollout.session == *session => (rollout.messages, Some(rollout.cwd)),
            _ => return Ok(out),
        }
    } else {
        (transcript::parse(&bytes), None)
    };
    let marked: Vec<(usize, Vec<Mark>)> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == Speaker::User && m.text().contains(capture::MARKER))
        .map(|(i, m)| (i, annotation::marks(&m.text())))
        .filter(|(_, marks)| !marks.is_empty())
        .collect();
    if marked.is_empty() {
        return Ok(out);
    }
    let store = ctx.store();
    let records = store.marks(session)?;
    let unsettled = |message: &Message, mark: &Mark| {
        !records
            .iter()
            .any(|r| r.is(&message.uuid, mark.number) && r.outcome.is_final())
    };
    let candidates: BTreeSet<&str> = marked
        .iter()
        .filter(|(i, marks)| marks.iter().any(|m| unsettled(&messages[*i], m)))
        .map(|(i, _)| messages[*i].uuid.as_str())
        .collect();
    if candidates.is_empty() {
        return Ok(out);
    }
    let raw = if codex_cwd.is_some() {
        BTreeMap::new()
    } else {
        raw_lines(&bytes, &candidates)
    };
    let mut work = Vec::new();
    for (index, marks) in marked {
        let message = &messages[index];
        if !candidates.contains(message.uuid.as_str()) {
            continue;
        }
        // Only a turn he plainly typed is his: not an agent or harness
        // wrapper, not a session control. Codex's rollout keeps only
        // host-attributed typed inputs.
        let cwd = match &codex_cwd {
            Some(cwd) => {
                if super::codex::is_control(&message.text()) {
                    continue;
                }
                cwd.clone()
            }
            None => {
                let Some(prompt) = raw
                    .get(&message.uuid)
                    .and_then(|value| prompt_of(value, message, session, payload).ok())
                else {
                    continue;
                };
                if payload::agent_originated(&prompt)
                    || scribe_core::control::parse(&prompt.prompt).is_some()
                {
                    continue;
                }
                prompt.cwd
            }
        };
        if transcript::daniel_turn(message).is_none() || cwd.is_empty() {
            continue;
        }
        let turn = Turn {
            session: session.clone(),
            at: message.at,
            prompt_sha256: capture::prompt_sha256(&message.text()),
            project: project::of(std::path::Path::new(&cwd)),
            cwd,
            provider: Label::sanitized(ctx.provider().as_str(), ctx.provider().as_str()),
            host: ctx.host(),
        };
        let written = written_lines(ctx, session, message)?;
        let (refused, open): (Vec<Mark>, Vec<Mark>) = marks
            .into_iter()
            .filter(|m| unsettled(message, m) && !written.contains(&m.line))
            .partition(|m| m.problem.is_some());
        let refused = refused
            .into_iter()
            .filter(|m| !records.iter().any(|r| r.is(&message.uuid, m.number)))
            .collect::<Vec<_>>();
        if !refused.is_empty() || !open.is_empty() {
            work.push(Work {
                message,
                index,
                turn,
                refused,
                open,
            });
        }
    }
    if work.is_empty() {
        return Ok(out);
    }

    let config = match ctx.config() {
        Ok(config) => config,
        Err(err) => {
            out.echoes.push(format!(
                "scribe: could not catalog #scribe in your message — {err}; will retry at the next reply"
            ));
            out.outcomes.push(refusal(session, err.code()));
            return Ok(out);
        }
    };
    let redactor = match ctx.redactor(&config) {
        Ok(redactor) => redactor,
        Err(err) => {
            out.echoes.push(format!(
                "scribe: could not catalog #scribe in your message — {err}; will retry at the next reply"
            ));
            out.outcomes.push(refusal(session, err.code()));
            return Ok(out);
        }
    };

    let mut models = Some(models);
    let mut adapter: Option<M> = None;
    let mut updates: Vec<(MarkKey, MarkChange)> = Vec::new();
    let mut says: Vec<Say> = Vec::new();
    let mut calls = 0usize;
    for item in &work {
        let key = |mark: &Mark| MarkKey {
            session: session.clone(),
            message: item.message.uuid.clone(),
            prompt_sha256: item.turn.prompt_sha256.clone(),
            marker: mark.number,
        };
        for mark in &item.refused {
            let code = mark.problem.as_ref().map_or(MODEL_FAILED, |p| p.code());
            updates.push((key(mark), MarkChange::Refused(code)));
            says.push(Say::Silent);
        }
        if item.open.is_empty() || calls >= MAX_TURNS_PER_STOP {
            continue;
        }
        calls += 1;
        let window = annotation::window(&messages, &item.message.uuid);
        let in_window: BTreeSet<&str> = window.iter().map(|m| m.uuid.as_str()).collect();
        let typed: Vec<&Message> = messages[..=item.index]
            .iter()
            .filter(|m| in_window.contains(m.uuid.as_str()))
            .filter(|m| transcript::daniel_turn(m).is_some())
            .collect();
        let open: Vec<&Mark> = item.open.iter().collect();
        let request = annotation::request(&open, window, &config.tags, &config.models.annotation);
        if adapter.is_none() {
            adapter = models.take().map(|build| build(&config));
        }
        let answer = match &adapter {
            Some(adapter) => adapter.annotation(&request).map_err(|e| e.to_string()),
            None => Err(String::from("no model adapter")),
        };
        for mark in &item.open {
            let found = answer
                .as_ref()
                .map(|a| a.marks.iter().find(|m| m.marker == mark.number));
            let (change, say) = match found {
                Err(err) => (MarkChange::Failed, Say::Failed(err.clone())),
                Ok(None) => (
                    MarkChange::Failed,
                    Say::Failed(format!("the answer had no marker {}", mark.number)),
                ),
                Ok(Some(answer)) => match annotation::check(mark, answer, &typed, &config.tags) {
                    Ok(Checked::Entry(annotated)) => {
                        let mut draft = annotation::draft(
                            mark,
                            annotated,
                            &item.turn,
                            &item.message.uuid,
                            &config.tags,
                        );
                        // Under an action tag, the §4.6 window, model-free.
                        if config.tags.action(&draft.tag).is_some() {
                            draft.window = Some(capture::action_window(
                                &messages,
                                &item.turn.prompt_sha256,
                                &item.message.text(),
                                item.turn.at,
                            ));
                        }
                        (MarkChange::Write(Box::new(draft)), Say::Answer)
                    }
                    Ok(Checked::None(reason)) => (MarkChange::Nothing, Say::Reason(reason)),
                    Err(refused) => (
                        MarkChange::Refused(refused.code()),
                        Say::Reason(refused.message().to_owned()),
                    ),
                },
            };
            updates.push((key(mark), change));
            says.push(say);
        }
    }
    if updates.is_empty() {
        return Ok(out);
    }
    let results = match store.record_marks(updates, &redactor, ctx.now()) {
        Ok(results) => results,
        Err(err) => {
            out.echoes.push(format!(
                "scribe: could not catalog #scribe in your message — {}; will retry at the next reply",
                chain(&err)
            ));
            out.outcomes.push(refusal(session, err.code()));
            return Ok(out);
        }
    };
    for (result, say) in results.into_iter().zip(says) {
        echo(&mut out, session, result, say);
    }
    Ok(out)
}

/// The echo and audit outcome of one recorded marker.
fn echo(out: &mut Cataloged, session: &SessionId, result: MarkResult, say: Say) {
    let nothing = |reason: &str| {
        format!(
            "scribe: nothing recorded for #scribe in your message — {}",
            quote(reason)
        )
    };
    match (result, say) {
        (MarkResult::Already(_), _) => {
            // A marker a concurrent Stop settled: audited by that Stop.
        }
        (MarkResult::Refused(code), Say::Silent) => {
            // The prompt hook echoed this refusal; it is audited here, once.
            out.outcomes.push(refusal(session, code));
        }
        (_, Say::Silent) => {}
        (MarkResult::Added(entry), _) => {
            out.echoes.push(format!(
                "scribe: added {} to {} — \"{}\" · your words: \"{}\"",
                entry.id,
                entry.tag,
                quote(&entry.content),
                quote(entry.quote.as_deref().unwrap_or_default())
            ));
            out.outcomes.push(HookOutcome {
                target: Some(entry.id.to_string()),
                error_code: None,
                issue: None,
            });
        }
        (MarkResult::Covered(entry), _) => {
            out.echoes.push(nothing(&format!(
                "already covered by {} in {}",
                entry.id, entry.tag
            )));
            out.outcomes.push(refusal(session, COVERED));
        }
        (MarkResult::NeedsRedaction, _) => {
            out.echoes
                .push(nothing("your words hold text redaction would mask"));
            out.outcomes.push(refusal(session, NEEDS_REDACTION));
        }
        (MarkResult::Nothing, say) => {
            let reason = say
                .text()
                .unwrap_or_else(|| String::from("nothing to record"));
            out.echoes.push(nothing(&reason));
            out.outcomes.push(HookOutcome {
                target: Some(format!("session:{session}")),
                error_code: None,
                issue: None,
            });
        }
        (MarkResult::Refused(code), say) => {
            let reason = say.text().unwrap_or_else(|| String::from(code));
            out.echoes.push(nothing(&reason));
            out.outcomes.push(refusal(session, code));
        }
        (MarkResult::Failed { attempts, gave_up }, say) => {
            let err = say
                .text()
                .unwrap_or_else(|| String::from("the model call failed"));
            if gave_up {
                out.echoes.push(format!(
                    "scribe: could not catalog #scribe in your message — {}; gave up after {MAX_ATTEMPTS} attempts",
                    quote(&err)
                ));
                out.outcomes.push(refusal(session, MODEL_FAILED));
            } else {
                // Pending: retried at the next reply, audited when settled.
                out.echoes.push(format!(
                    "scribe: could not catalog #scribe in your message — {}; will retry at the next reply ({attempts} of {MAX_ATTEMPTS})",
                    quote(&err)
                ));
            }
        }
    }
}

fn refusal(session: &SessionId, code: &'static str) -> HookOutcome {
    HookOutcome {
        target: Some(format!("session:{session}")),
        error_code: Some(code),
        issue: None,
    }
}

/// The lines of `message` that already hold an entry: lines recorded
/// verbatim before markers took them over (§14.6). A prompt hook may have
/// received one text block rather than the joined message, so each block's
/// cites are translated into the joined text's line numbers too.
fn written_lines(
    ctx: &Context,
    session: &SessionId,
    message: &Message,
) -> Result<BTreeSet<u32>, CliError> {
    let store = ctx.store();
    let mut written: BTreeSet<u32> = store
        .entries_from_prompt(session, &capture::prompt_sha256(&message.text()))?
        .iter()
        .filter_map(|entry| entry.cite.as_ref().map(|cite| cite.line))
        .collect();
    if message.blocks.len() > 1 {
        let mut offset = 0u32;
        for block in &message.blocks {
            for entry in store.entries_from_prompt(session, &capture::prompt_sha256(block))? {
                if let Some(cite) = entry.cite {
                    written.insert(offset.saturating_add(cite.line));
                }
            }
            offset = offset
                .saturating_add(u32::try_from(block.split('\n').count()).unwrap_or(u32::MAX))
                .saturating_add(1);
        }
    }
    Ok(written)
}

/// The raw transcript line of each `candidates` message, by uuid, kept for
/// its original origin fields and cwd, which parsing the conversation
/// deliberately strips.
fn raw_lines(bytes: &[u8], candidates: &BTreeSet<&str>) -> BTreeMap<String, serde_json::Value> {
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
    raw
}

/// The prompt payload a transcript user message would have had: its raw
/// line with the session, transcript path and joined text, and the Stop
/// payload's cwd when the line has none (the empty placeholder lets the
/// origin guard skip synthetic turns even without cwd; it must never reach
/// capture as a citation).
fn prompt_of(
    raw: &serde_json::Value,
    message: &transcript::Message,
    session: &SessionId,
    payload: &StopPayload,
) -> serde_json::Result<PromptPayload> {
    let mut value = raw.clone();
    value["session_id"] = serde_json::json!(session);
    value["transcript_path"] = serde_json::json!(payload.transcript_path);
    value["prompt"] = serde_json::json!(message.text());
    if value["cwd"].is_null() {
        value["cwd"] = serde_json::json!(payload.cwd.as_deref().unwrap_or_default());
    }
    serde_json::from_value(value)
}
