//! Catalog the `#scribe` markers the user typed inside their sentences, at the
//! Stop of the turn (docs/design/scribe/2_design.md §14; ORB-27707 step 3b).
//!
//! The prompt hook only echoed the marker. Here, after the reply, each
//! typed turn of the transcript with a marker that `marks.json` has not
//! settled gets one bounded `models.annotation` call ([`MAX_TURNS_PER_STOP`]
//! turns per Stop, oldest first; `scribe mark retry` runs the same catalog
//! for one session with no per-run bound). A typed turn is one the shared
//! typed-turn rule keeps (`instruction::claude_conversation_raw_typed`, §4.1):
//! a plain prompt, a queued line Daniel typed while the agent was busy, or a
//! slash command, whose markers are read from its args
//! (`annotation::turn_marks`). Each answer is checked without a model
//! (`annotation::check`) and recorded in one store call that writes the
//! entries and `marks.json` (`Store::record_marks`):
//!
//! - an entry: the user's, content scribe's statement, `quote` their words,
//!   `stated_by` scribe:annotation, no proposal;
//! - none: nothing written, never asked again;
//! - refused (a check, a cover, redaction, or a marker refused before any
//!   call): never asked again;
//! - a failed call: nothing written, the marker stays pending and the next
//!   Stop retries it, up to [`MAX_ATTEMPTS`] calls, then refused
//!   `model_failed`. A given-up marker stays listed (`scribe mark list`)
//!   until `scribe mark retry` catalogs it, and its echo also reaches the
//!   agent (`Cataloged::context`) with the line, so the agent can record
//!   the user's words itself.
//!
//! The user sees an echo for every outcome except a marker the prompt hook
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

use crate::annotation::{self, Checked, MAX_ATTEMPTS, Mark};
use crate::capture::{self, Turn};
use crate::cases::hook::{HookError, HookOutcome, chain, is_control, quote, refusal};
use crate::cases::mark::MarkVerbs;
use crate::cases::{Ctx, PortError, TranscriptState};
use crate::transcript::{self, Message};
use crate::{
    COVERED, Config, Label, MODEL_FAILED, MarkChange, MarkKey, MarkResult, Models, NEEDS_REDACTION,
    Provider, SessionId, Speaker,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What `run` catalogs: one session's Stop, or `scribe mark retry`.
pub struct AnnotationInput<'i, F> {
    /// The session.
    pub session: &'i SessionId,
    /// Its transcript.
    pub transcript_path: &'i Path,
    /// The cwd the Stop payload named, for a line that carries none.
    pub cwd: Option<String>,
    /// The provider whose transcript it is.
    pub provider: Provider,
    /// This host, as records name it.
    pub host: Label,
    /// The most marked turns to call a model for.
    pub turns: usize,
    /// Builds the `Models` adapter for the config, only when a call is due.
    pub models: F,
}

/// What the hook tells the user and the agent, and audits.
#[derive(Debug, Default)]
pub struct Cataloged {
    /// For the user, one per marker outcome.
    pub echoes: Vec<String>,
    /// For the agent: one per given-up marker, with the line and what the
    /// agent may do instead.
    pub context: Vec<String>,
    /// For the audit log: one per write or refusal.
    pub outcomes: Vec<HookOutcome>,
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
    /// The call's error, for a failure, and the marker's line as typed.
    Failed { error: String, said: String },
    /// The model's answer.
    Answer,
}

impl Say {
    /// Its words, when it has any.
    fn text(self) -> Option<String> {
        match self {
            Self::Reason(text) | Self::Failed { error: text, .. } => Some(text),
            Self::Silent | Self::Answer => None,
        }
    }
}

/// Catalog the session's unsettled markers, at most `turns` marked turns
/// (oldest first), building the model adapter with `models` only when a
/// call is due.
///
/// # Errors
///
/// [`HookError`]: a store read failed or the transcript could not be read.
/// A config, redactor or write failure is echoed and audited instead.
pub fn run<E: PortError, M: Models>(
    ctx: &Ctx<'_, E>,
    input: AnnotationInput<'_, impl FnOnce(&Config) -> M>,
) -> Result<Cataloged, HookError<E>> {
    let AnnotationInput {
        session,
        transcript_path,
        cwd: payload_cwd,
        provider,
        host,
        turns,
        models,
    } = input;
    let mut out = Cataloged::default();
    // Stop checked the transcript first and reported a failure.
    if !matches!(
        ctx.transcripts.check(provider, transcript_path, session),
        Ok(TranscriptState::Present)
    ) {
        return Ok(out);
    }
    let bytes = ctx
        .transcripts
        .read(provider, transcript_path)
        .map_err(|source| HookError::Read {
            what: transcript_path.display().to_string(),
            source,
        })?;
    // Codex: the rollout's verified typed inputs and assistant text, and
    // the session's cwd; the dispatcher already checked its identity.
    let (messages, codex_cwd, line_indices) = if provider == Provider::Codex {
        match crate::codex::parse(&bytes) {
            Ok(rollout) if rollout.session == *session => {
                (rollout.messages, Some(rollout.cwd), None)
            }
            _ => return Ok(out),
        }
    } else {
        let (messages, line_indices) = transcript::parse_with_line_indices(&bytes);
        (messages, None, line_indices)
    };
    let marked: Vec<(usize, Vec<Mark>)> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == Speaker::User && m.text().contains(capture::MARKER))
        .map(|(i, m)| (i, annotation::turn_marks(&m.text())))
        .filter(|(_, marks)| !marks.is_empty())
        .collect();
    if marked.is_empty() {
        return Ok(out);
    }
    let store = ctx.store;
    let records = store.read().marks().of_session(session)?;
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
    let raw = match (codex_cwd.as_ref(), line_indices.as_deref()) {
        (None, Some(indices)) => raw_lines(&bytes, &candidates, &messages, indices),
        _ => BTreeMap::new(),
    };
    // The Claude user turns Daniel typed, by the rule every typed-turn reader
    // shares (§4.1): the line's origin claims, then his words (a slash
    // command's args, not its harness tags) free of agent wrappers.
    let typed: BTreeSet<String> = if codex_cwd.is_none() {
        crate::instruction::claude_conversation_raw_typed(&bytes)
            .into_iter()
            .filter(|spoken| spoken.typed.is_some())
            .map(|spoken| spoken.message.uuid)
            .collect()
    } else {
        BTreeSet::new()
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
                if is_control(&message.text()) {
                    continue;
                }
                cwd.clone()
            }
            None => {
                if !typed.contains(&message.uuid)
                    || crate::control::parse(&message.text()).is_some()
                {
                    continue;
                }
                // The line's own cwd, else the Stop payload's.
                raw.get(&message.uuid)
                    .and_then(|value| value.get("cwd"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|cwd| !cwd.is_empty())
                    .map(str::to_owned)
                    .or_else(|| payload_cwd.clone())
                    .unwrap_or_default()
            }
        };
        if transcript::user_turn(message).is_none() || cwd.is_empty() {
            continue;
        }
        let turn = Turn {
            session: session.clone(),
            at: message.at,
            prompt_sha256: capture::prompt_sha256(&message.text()),
            project: ctx.transcripts.project_of(Path::new(&cwd)),
            cwd,
            provider: Label::sanitized(provider.as_str(), provider.as_str()),
            host: host.clone(),
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

    let config = match store.read().tags().config() {
        Ok(config) => config.for_provider(provider),
        Err(err) => {
            out.echoes.push(format!(
                "scribe: could not catalog #scribe in your message — {err}; will retry at the next reply"
            ));
            out.outcomes.push(refusal(session, err.code()));
            return Ok(out);
        }
    };
    let redactor = match ctx.redaction.redactor(&config) {
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
        let open: Vec<&Mark> = item
            .open
            .iter()
            .filter(|mark| {
                if let Some(refused) = annotation::record_tag_refusal(mark, &config.tags) {
                    updates.push((key(mark), MarkChange::Refused(refused.code())));
                    says.push(Say::Reason(refused.message().to_owned()));
                    false
                } else {
                    true
                }
            })
            .collect();
        if open.is_empty() || calls >= turns {
            continue;
        }
        calls += 1;
        let window = annotation::window(&messages, &item.message.uuid);
        let in_window: BTreeSet<&str> = window.iter().map(|m| m.uuid.as_str()).collect();
        // The window's turns he typed: a quote must come from one of them.
        let own: Vec<&Message> = messages[..=item.index]
            .iter()
            .filter(|m| in_window.contains(m.uuid.as_str()))
            .filter(|m| transcript::user_turn(m).is_some())
            .filter(|m| codex_cwd.is_some() || typed.contains(&m.uuid))
            .collect();
        let request = annotation::request(
            &open,
            window,
            &config.tags,
            &config.models.annotation,
            config.voice(crate::voice::ModelUse::Annotation),
        );
        if adapter.is_none() {
            adapter = models.take().map(|build| build(&config));
        }
        let answer = match &adapter {
            Some(adapter) => adapter.annotation(&request).map_err(|e| e.to_string()),
            None => Err(String::from("no model adapter")),
        };
        for mark in open {
            let found = answer
                .as_ref()
                .map(|a| a.marks.iter().find(|m| m.marker == mark.number));
            let failed = |error: String| Say::Failed {
                error,
                said: mark.said.clone(),
            };
            let (change, say) = match found {
                Err(err) => (MarkChange::Failed, failed(err.clone())),
                Ok(None) => (
                    MarkChange::Failed,
                    failed(format!("the answer had no marker {}", mark.number)),
                ),
                Ok(Some(answer)) => match annotation::check(mark, answer, &own, &config.tags) {
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
    let results = match store.record_marks(updates, &redactor, ctx.clock.now()) {
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
            });
        }
        (MarkResult::Refused(code), say) => {
            let reason = say.text().unwrap_or_else(|| String::from(code));
            out.echoes.push(nothing(&reason));
            out.outcomes.push(refusal(session, code));
        }
        (MarkResult::Failed { attempts, gave_up }, say) => {
            let said = match &say {
                Say::Failed { said, .. } => said.clone(),
                Say::Silent | Say::Reason(_) | Say::Answer => String::new(),
            };
            let err = say
                .text()
                .unwrap_or_else(|| String::from("the model call failed"));
            if gave_up {
                let echo = format!(
                    "scribe: could not catalog #scribe in your message — {}; gave up after {MAX_ATTEMPTS} attempts",
                    quote(&err)
                );
                out.echoes
                    .push(format!("{echo} — scribe mark retry {session} tries again"));
                // The agent gets the line too: nothing was recorded, so it
                // may record the user's words itself.
                out.context.push(format!(
                    "{echo} — the line: \"{}\" {}",
                    quote(&said),
                    gave_up_note(session)
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

/// What the agent is told after a given-up marker: it was not recorded, and
/// how the user's words can still be kept.
fn gave_up_note(session: &SessionId) -> String {
    format!(
        "(not cataloged by scribe; if the user wants it kept, record it with their exact words: scribe entry add --tag <tag> --quote '<their words>' --spoken '<statement>' --author <name>, or ask them to run scribe mark retry {session} from a shell with model credentials)"
    )
}

/// The lines of `message` that already hold an entry: lines recorded
/// verbatim before markers took them over (§14.6). A prompt hook may have
/// received one text block rather than the joined message, so each block's
/// cites are translated into the joined text's line numbers too.
fn written_lines<E>(
    ctx: &Ctx<'_, E>,
    session: &SessionId,
    message: &Message,
) -> Result<BTreeSet<u32>, HookError<E>> {
    let store = ctx.store;
    let mut written: BTreeSet<u32> = store
        .read()
        .entries()
        .from_prompt(session, &capture::prompt_sha256(&message.text()))?
        .iter()
        .filter_map(|entry| entry.cite.as_ref().map(|cite| cite.line))
        .collect();
    if message.blocks.len() > 1 {
        let mut offset = 0u32;
        for block in &message.blocks {
            for entry in store
                .read()
                .entries()
                .from_prompt(session, &capture::prompt_sha256(block))?
            {
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

/// The raw source row selected by `transcript::parse` for each candidate user
/// message, kept for its origin fields and cwd, which parsing deliberately
/// strips.
fn raw_lines(
    bytes: &[u8],
    candidates: &BTreeSet<&str>,
    messages: &[Message],
    line_indices: &[usize],
) -> BTreeMap<String, serde_json::Value> {
    let selected = messages
        .iter()
        .zip(line_indices)
        .filter(|(message, _)| {
            message.role == Speaker::User && candidates.contains(message.uuid.as_str())
        })
        .map(|(message, line_index)| (*line_index, message.uuid.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut raw = BTreeMap::new();
    for (line_index, line) in bytes.split(|&b| b == b'\n').enumerate() {
        // Keep a DOM only for candidate user rows the parser retained, so
        // progress rows and replayed or skipped conversation rows cannot lend
        // origin metadata to another message with the same uuid.
        if let Some(uuid) = selected.get(&line_index)
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(line)
        {
            raw.insert(uuid.clone(), value);
        }
    }
    raw
}
