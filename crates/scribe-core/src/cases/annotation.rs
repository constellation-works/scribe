//! Hand the `#scribe` markers the user typed inside their sentences to the
//! agent, at the Stop of the turn (docs/design/scribe/2_design.md §14;
//! ORB-27911 step 2a removed the model call that cataloged them).
//!
//! The prompt hook only echoed the marker. Here, after the reply, each
//! typed turn of the transcript with a marker that `marks.json` has not
//! settled is read. A typed turn is one the shared typed-turn rule keeps
//! (`instruction::claude_conversation_raw_typed`, §4.1): a plain prompt, a
//! queued line the user typed while the agent was busy, or a slash command,
//! whose markers are read from its args (`annotation::turn_marks`). Each
//! unsettled marker is recorded in one store call (`Store::record_marks`):
//!
//! - refused before any handoff (a marker problem, or a decision or open
//!   item named): never handed over;
//! - otherwise refused `agent_handoff`: its line goes to the agent
//!   (`Cataloged::context`) with the `entry add` command that records it as
//!   the user's, and the user is told the agent has it. Settled: it is not
//!   handed over again.
//!
//! A turn with no marker, or whose markers are all settled, reads only the
//! transcript and `marks.json`: no lock, no write.
//!
//! A marker opening a line is a folded form 1–3 (§14.6): before store
//! format 22 such a line was recorded verbatim by the prompt hook, or by
//! Stop recovery, and its entry cites that prompt and line. A marker on a
//! line that already has an entry is left alone, so an older transcript
//! is not handed over twice.
//!
//! Under Codex the Stop reads Codex's own rollout (`scribe_core::codex`):
//! its user messages are the host-attributed typed inputs, its cwd the
//! session's, and a `$scribe` skill control is skipped as a `/scribe` one is.

use crate::annotation::{self, AGENT_HANDOFF, Mark};
use crate::capture::{self, Turn};
use crate::cases::hook::{HookError, HookOutcome, chain, quote, refusal};
use crate::cases::mark::MarkVerbs;
use crate::control::is_control;
use crate::runtime::{Ctx, PortError, TranscriptState};
use crate::transcript::{self, Message};
use crate::{Label, TagTable};
use crate::{MODEL_FAILED, MarkChange, MarkKey, MarkResult, Provider, SessionId, Speaker};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What `run` hands over: one session's Stop.
pub struct AnnotationInput<'i> {
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
}

/// What the hook tells the user and the agent, and audits.
#[derive(Debug, Default)]
pub struct Cataloged {
    /// For the user, one per marker outcome.
    pub echoes: Vec<String>,
    /// For the agent: one per handed-over marker, with the line and the
    /// command that records it.
    pub context: Vec<String>,
    /// For the audit log: one per handoff or refusal.
    pub outcomes: Vec<HookOutcome>,
}

/// One marked turn still to hand over.
struct Work<'a> {
    message: &'a Message,
    turn: Turn,
    /// Markers refused before any handoff and not yet recorded.
    refused: Vec<Mark>,
    /// Markers to hand to the agent.
    open: Vec<Mark>,
}

/// How one update's result is echoed.
enum Say {
    /// Recorded silently: the prompt hook already echoed it.
    Silent,
    /// Its reason, for a refusal the checks made.
    Reason(String),
    /// Handed to the agent: the line as typed, the `entry add` hints and
    /// whether the hinted tag takes `--spoken`.
    Handoff {
        said: String,
        hints: String,
        spoken: bool,
    },
}

/// Hand the session's unsettled markers to the agent, oldest first.
///
/// # Errors
///
/// [`HookError`]: a store read failed or the transcript could not be read.
/// A config or write failure is echoed and audited instead.
pub fn run<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: AnnotationInput<'_>,
) -> Result<Cataloged, HookError<E>> {
    let AnnotationInput {
        session,
        transcript_path,
        cwd: payload_cwd,
        provider,
        host,
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
                "scribe: could not hand over #scribe in your message — {err}; will retry at the next reply"
            ));
            out.outcomes.push(refusal(session, err.code()));
            return Ok(out);
        }
    };

    let mut updates: Vec<(MarkKey, MarkChange)> = Vec::new();
    let mut says: Vec<Say> = Vec::new();
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
        for mark in &item.open {
            if let Some(refused) = annotation::record_tag_refusal(mark, &config.tags) {
                updates.push((key(mark), MarkChange::Refused(refused.code())));
                says.push(Say::Reason(refused.message().to_owned()));
            } else {
                updates.push((key(mark), MarkChange::Refused(AGENT_HANDOFF)));
                says.push(Say::Handoff {
                    said: mark.said.clone(),
                    hints: hints(mark, &item.turn, &config.tags),
                    spoken: spoken(mark, &config.tags),
                });
            }
        }
    }
    if updates.is_empty() {
        return Ok(out);
    }
    let results = match store.record_marks(updates, ctx.clock.now()) {
        Ok(results) => results,
        Err(err) => {
            out.echoes.push(format!(
                "scribe: could not hand over #scribe in your message — {}; will retry at the next reply",
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

/// The `entry add` flags the marker settles: its tag when it names one, its
/// project (`global` for `#scribe/global`) and its hashtags as topics.
fn hints(mark: &Mark, turn: &Turn, tags: &TagTable) -> String {
    let mut out = format!(
        "--tag {}",
        mark.hint(tags)
            .map_or_else(|| String::from("<tag>"), ToString::to_string)
    );
    let project = if mark.global {
        String::from("global")
    } else {
        turn.project.to_string()
    };
    out.push_str(&format!(" --project {project}"));
    for topic in mark.topics.as_slice() {
        out.push_str(&format!(" --topic {topic}"));
    }
    out
}

/// Whether `entry add --spoken` takes the marker's hinted tag: an ordinary
/// memory tag, or no hint. It refuses an action, evidence or handoff tag
/// (`not_memory_tag`), so their command records the quote without it.
fn spoken(mark: &Mark, tags: &TagTable) -> bool {
    mark.hint(tags)
        .and_then(|name| tags.get(name))
        .is_none_or(|def| {
            def.action.is_none()
                && !def.require_evidence
                && def.name.as_str() != crate::handoff::HANDOFF_TAG
        })
}

/// The echo and audit outcome of one recorded marker.
fn echo(out: &mut Cataloged, session: &SessionId, result: MarkResult, say: Say) {
    match (result, say) {
        (MarkResult::Already(_), _) => {
            // A marker a concurrent Stop settled: audited by that Stop.
        }
        (MarkResult::Refused(code), Say::Silent) => {
            // The prompt hook echoed this refusal; it is audited here, once.
            out.outcomes.push(refusal(session, code));
        }
        (
            MarkResult::Refused(_),
            Say::Handoff {
                said,
                hints,
                spoken,
            },
        ) => {
            let statement = if spoken {
                "--spoken '<statement>'"
            } else {
                "'<statement>'"
            };
            out.echoes.push(format!(
                "scribe: #scribe in your message is the agent's to record — \"{}\"",
                quote(&said)
            ));
            out.context.push(format!(
                "scribe: the user marked a line with #scribe for you to record — \"{}\". If it marks something to keep, record it as theirs with their exact words: scribe entry add {hints} --author <name> --related session:{session} --quote '<their words>' {statement}. If it marks nothing, record nothing.",
                quote(&said)
            ));
            out.outcomes.push(refusal(session, AGENT_HANDOFF));
        }
        (MarkResult::Refused(code), Say::Reason(reason)) => {
            out.echoes.push(format!(
                "scribe: nothing recorded for #scribe in your message — {}",
                quote(&reason)
            ));
            out.outcomes.push(refusal(session, code));
        }
    }
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
