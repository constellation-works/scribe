//! `scribe hook user-prompt-submit`: record each `#scribe` line of a typed
//! prompt (docs/design/scribe/2_design.md §4.1–§4.4).
//!
//! A prompt with no `#scribe` line returns at once: nothing is read, written,
//! audited or printed. Otherwise the transcript provenance is checked, the
//! config loaded, every line parsed, and the parsed ones recorded in one
//! store call, which finds an entry already written for the same line instead
//! of writing it again. The reply tells Daniel (`systemMessage`) and the main
//! agent (`additionalContext`) the same thing, one line per `#scribe` line,
//! and never blocks the prompt: a refusal or a store failure is a reply, not
//! an error exit, so the rest of the prompt still reaches the agent.

use super::payload::{self, PromptPayload};
use super::project;
use crate::app::Context;
use crate::error::CliError;
use crate::output::{HookOutcome, HookReply, Output};
use scribe_core::capture::{self, Parsed, Turn};
use scribe_core::{Captured, Label, SessionId};
use std::collections::BTreeMap;

/// The hook event this replies to.
const EVENT: &str = "UserPromptSubmit";
/// The tool the entries are recorded through.
const PROVIDER: &str = "claude-code";
/// The longest quote an echo shows, in characters; `entry show` has it all.
const ECHO_CHARS: usize = 200;
/// What the agent is told after the echo, so it does not record it again.
const AGENT_NOTE: &str = "(recorded by scribe; no action needed)";
/// The code of a payload whose transcript failed the provenance check.
const PROVENANCE_FAILED: &str = "provenance_failed";

/// Run the hook on its stdin payload.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let payload: PromptPayload = payload::decode(input)?;
    let session = payload::session(&payload.session_id)?;
    let lines = capture::lines(&payload.prompt);
    if lines.is_empty() {
        return Ok(reply(Vec::new(), Vec::new()));
    }
    let failed_all = |message: &str, code: &'static str| {
        let echoes = lines
            .iter()
            .map(|(_, said)| format!("scribe: could not record \"{}\" — {message}", quote(said)))
            .collect();
        let outcomes = lines.iter().map(|_| refusal(&session, code)).collect();
        Ok(reply(echoes, outcomes))
    };

    // A missing transcript passes: the first prompt of a session can
    // arrive before Claude Code has written the file.
    if let Err(reason) =
        payload::check_transcript(&payload.transcript_path, &session, ctx.transcripts())
    {
        return failed_all(
            &format!("the hook payload failed the transcript check: {reason}"),
            PROVENANCE_FAILED,
        );
    }
    let config = match ctx.store().config() {
        Ok(config) => config,
        Err(err) => return failed_all(&chain(&err), err.code()),
    };

    let at = ctx.now();
    let turn = Turn {
        session: session.clone(),
        at,
        prompt_sha256: capture::prompt_sha256(&payload.prompt),
        cwd: payload.cwd.clone(),
        project: project::of(std::path::Path::new(&payload.cwd)),
        provider: Label::sanitized(PROVIDER, PROVIDER),
        host: ctx.host(),
    };
    let parsed = capture::scan(&payload.prompt, &config.tags);
    let drafts: Vec<_> = parsed
        .iter()
        .filter_map(|p| match p {
            Parsed::Instruction(i) => Some(capture::draft(i, &turn, &config.tags)),
            Parsed::Refused(_) => None,
        })
        .collect();
    let stored = ctx.store().capture(drafts, &ctx.redactor(&config));

    let mut captured: BTreeMap<u32, Captured> = BTreeMap::new();
    let mut store_error: Option<scribe_core::Error> = None;
    match stored {
        Ok(results) => {
            for result in results {
                captured.insert(result.entry().cite.line, result);
            }
        }
        Err(err) => store_error = Some(err),
    }

    let mut echoes = Vec::new();
    let mut outcomes = Vec::new();
    for item in &parsed {
        match item {
            Parsed::Refused(refused) => {
                echoes.push(format!(
                    "scribe: refused \"{}\" — {}",
                    quote(&refused.said),
                    refused.reason
                ));
                outcomes.push(refusal(&session, refused.reason.code()));
            }
            Parsed::Instruction(instruction) => {
                match (captured.get(&instruction.line), &store_error) {
                    (Some(Captured::Added(entry)), _) => {
                        echoes.push(format!(
                            "scribe: added {} to {} — \"{}\"",
                            entry.id,
                            entry.tag,
                            quote(&entry.content)
                        ));
                        outcomes.push(HookOutcome {
                            target: Some(entry.id.to_string()),
                            error_code: None,
                        });
                    }
                    (Some(Captured::Existing(entry)), _) => echoes.push(format!(
                        "scribe: already recorded {} in {} — \"{}\"",
                        entry.id,
                        entry.tag,
                        quote(&entry.content)
                    )),
                    (None, Some(err)) => {
                        echoes.push(format!(
                            "scribe: could not record \"{}\" — {}",
                            quote(&instruction.said),
                            chain(err)
                        ));
                        outcomes.push(refusal(&session, err.code()));
                    }
                    (None, None) => {}
                }
            }
        }
    }
    Ok(reply(echoes, outcomes))
}

fn refusal(session: &SessionId, code: &'static str) -> HookOutcome {
    HookOutcome {
        target: Some(format!("session:{session}")),
        error_code: Some(code),
    }
}

/// The reply: nothing at all when there is nothing to echo.
fn reply(echoes: Vec<String>, outcomes: Vec<HookOutcome>) -> Output {
    let (message, context) = if echoes.is_empty() {
        (None, None)
    } else {
        let text = echoes.join("\n");
        let context = format!("{text}\n{AGENT_NOTE}");
        (Some(text), Some(context))
    };
    Output::Hook(HookReply {
        event: EVENT,
        message,
        context,
        outcomes,
    })
}

/// `text` on one line, cut to [`ECHO_CHARS`].
fn quote(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= ECHO_CHARS {
        flat
    } else {
        let mut cut: String = flat.chars().take(ECHO_CHARS).collect();
        cut.push('…');
        cut
    }
}

/// An error and its causes on one line.
fn chain(err: &scribe_core::Error) -> String {
    let mut message = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}
