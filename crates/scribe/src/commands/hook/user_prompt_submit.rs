//! `scribe hook user-prompt-submit`: echo the `#scribe` markers of a typed
//! prompt and apply its settling commands (docs/design/scribe/2_design.md
//! §4.4, §14).
//!
//! A turn that is not plainly typed by the user (an agent or harness wrapper
//! such as `<agent-message>`, `<task-notification>`,
//! `<side-sessions-event>` or a scheduled/loop wrapper, or a non-plain
//! payload origin field; see `payload::agent_originated`) is skipped at
//! once: it echoes nothing, settles nothing and does not run `/scribe`.
//!
//! Every such turn is also matched, without a model, against the `issue`
//! tag's triggers (2_design.md §11.4, `scribe_core::issue_trigger::fire`),
//! in the user's own words only (pasted, fenced, indented, quoted and
//! `#scribe` lines left out), whether or not the session is on. On a match
//! that passes the transcript check the agent gets one more
//! `additionalContext` line (asking for a root cause: search issues first,
//! record the answer with `issue add` or `issue occur`), the user sees
//! nothing, and the fire is one audit line: the session, the trigger
//! phrase and the prompt's sha256, never its words. A config that does not
//! load gives no hint.
//!
//! Every `#scribe` the user typed, inside a sentence or opening a line
//! (`#scribe idea: …`, `#scribe add this to preferences`), is a marker
//! (§14.6): this hook writes nothing and calls no model for it, it only
//! echoes that the marker is noted; the Stop of this turn catalogs it
//! (`annotation.rs`). Under Codex the adapter (`codex.rs`) echoes them
//! instead, once it has verified the typed turn.
//!
//! A settling line (`#scribe approve|reject <p-id>`,
//! `#scribe retire|forget <entry-id>`, form 4) is a whole-line command, not
//! a marker: after the transcript check it goes to `proposal_command.rs`
//! (or `issue_command.rs` for an issue id) and is echoed in its place.
//!
//! A prompt whose first line is `/scribe on`, `/scribe off` or
//! `/scribe status` is not a turn: `scribe_command.rs` switches the session
//! and blocks it (§5.1), and its `#scribe` lines, if any, are not read.
//!
//! The reply tells the user (`systemMessage`) one echo per settling line and
//! one for the markers. The main agent (`additionalContext`) gets the same
//! echoes, each with a note: applied lines say no action is needed,
//! refused or failed lines say they must be relayed to the user, and markers
//! say scribe catalogs them after the reply. The hook never blocks the
//! prompt: a refusal or a store failure is a reply, not an error exit, so
//! the rest of the prompt still reaches the agent.

use super::payload::{self, PromptPayload};
use super::project;
use crate::app::Context;
use crate::error::CliError;
use crate::output::{HookOutcome, HookReply, Output};
use scribe_core::capture::{self, Parsed, Turn};
use scribe_core::trigger::Fired;
use scribe_core::{Label, Provider, SessionId, annotation};

/// The hook event this replies to.
const EVENT: &str = "UserPromptSubmit";
/// The longest quote an echo shows, in characters; `entry show` has it all.
const ECHO_CHARS: usize = 200;
/// What the agent is told after an applied echo, so it does not act on it
/// again.
const AGENT_NOTE: &str = "(recorded by scribe; no action needed)";
/// What the agent is told after a refused or failed echo: nothing was
/// applied, so the line must not be silently lost.
const AGENT_RELAY_NOTE: &str = "(not recorded by scribe; relay this to the user)";
/// What the agent is told after a marked echo: Stop catalogs the marker
/// after the reply, so the agent must not record it as well.
const AGENT_MARKED_NOTE: &str =
    "(scribe catalogs this #scribe marker after your reply; do not record it yourself)";
/// The code of a payload whose transcript failed the provenance check.
const PROVENANCE_FAILED: &str = "provenance_failed";

/// Run the hook on its stdin payload.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let payload: PromptPayload = payload::decode(input)?;
    capture_prompt(&payload, ctx)
}

/// One echo line and the note the agent gets for it.
struct Echo {
    text: String,
    note: &'static str,
}

impl Echo {
    fn recorded(text: String) -> Self {
        Self {
            text,
            note: AGENT_NOTE,
        }
    }

    fn not_recorded(text: String) -> Self {
        Self {
            text,
            note: AGENT_RELAY_NOTE,
        }
    }

    fn marked(text: String) -> Self {
        Self {
            text,
            note: AGENT_MARKED_NOTE,
        }
    }

    /// The echo of a settling command's reply: recorded unless its
    /// outcome carries a code (a refusal or failure).
    fn settled(echo: String, outcome: Option<&HookOutcome>) -> Self {
        if outcome.is_some_and(|o| o.error_code.is_some()) {
            Self::not_recorded(echo)
        } else {
            Self::recorded(echo)
        }
    }
}

fn capture_prompt(payload: &PromptPayload, ctx: &Context) -> Result<Output, CliError> {
    let session = payload::session(&payload.session_id)?;
    if ctx
        .store()
        .control()?
        .get(&session)
        .is_some_and(|c| c.provider != ctx.provider())
    {
        return Err(CliError::Usage(String::from(
            "session belongs to another transcript provider",
        )));
    }
    if payload::agent_originated(payload) {
        return Ok(reply(Vec::new(), Vec::new()));
    }
    if let Some(switch) = scribe_core::control::parse(&payload.prompt) {
        return Ok(super::scribe_command::run(&switch, payload, &session, ctx));
    }
    let fired = ctx.config().ok().and_then(|config| {
        scribe_core::issue_trigger::fire(&config.tags.issue_capture(), &payload.prompt)
    });
    // Markers (§14.6): echoed here, written nothing; the Stop of this turn
    // catalogs them. Under Codex the adapter echoes them once the typed
    // turn is verified (`codex.rs`, [`mark_echoes`]), since its Stop replays
    // turns through here.
    let mark_echoes = if ctx.provider() == Provider::Codex {
        Vec::new()
    } else {
        marked(&payload.prompt)
    };
    let parsed = capture::scan(&payload.prompt);
    if parsed.is_empty() {
        let output = reply(mark_echoes, Vec::new());
        return Ok(match fired {
            Some(fired) if payload::check(ctx, &payload.transcript_path, &session).is_ok() => {
                with_hint(output, &fired, &session, &payload.prompt)
            }
            _ => output,
        });
    }

    // A missing transcript passes: the first prompt of a session can
    // arrive before Claude Code has written the file.
    if let Err(reason) = payload::check(ctx, &payload.transcript_path, &session) {
        let echoes = parsed
            .iter()
            .map(|item| {
                Echo::not_recorded(format!(
                    "scribe: could not record \"{}\" — the hook payload failed the transcript check: {reason}",
                    quote(said_of(item))
                ))
            })
            .collect();
        let outcomes = parsed
            .iter()
            .map(|_| refusal(&session, PROVENANCE_FAILED))
            .collect();
        return Ok(reply(echoes, outcomes));
    }

    let turn = Turn {
        session: session.clone(),
        at: ctx.now(),
        prompt_sha256: capture::prompt_sha256(&payload.prompt),
        cwd: payload.cwd.clone(),
        project: project::of(std::path::Path::new(&payload.cwd)),
        provider: Label::sanitized(ctx.provider().as_str(), ctx.provider().as_str()),
        host: ctx.host(),
    };
    let mut echoes = Vec::new();
    let mut outcomes = Vec::new();
    for item in parsed {
        match item {
            Parsed::Refused(refused) => {
                echoes.push(Echo::not_recorded(format!(
                    "scribe: refused \"{}\" — {}",
                    quote(&refused.said),
                    refused.message()
                )));
                outcomes.push(refusal(&session, refused.reason.code()));
            }
            Parsed::Settle(settling) => {
                let reply = super::proposal_command::run(&settling, &turn, ctx);
                echoes.push(Echo::settled(reply.echo, reply.outcome.as_ref()));
                outcomes.extend(reply.outcome);
            }
            Parsed::ForgetIssue(forget) => {
                let reply = super::issue_command::run(&forget, &turn, ctx);
                echoes.push(Echo::settled(reply.echo, reply.outcome.as_ref()));
                outcomes.extend(reply.outcome);
            }
        }
    }
    echoes.extend(mark_echoes);
    // The transcript check passed above; a failed one returned before.
    let output = reply(echoes, outcomes);
    Ok(match fired {
        Some(fired) => with_hint(output, &fired, &session, &payload.prompt),
        None => output,
    })
}

/// The line a parsed settling command came from, as typed.
fn said_of(item: &Parsed) -> &str {
    match item {
        Parsed::Settle(settling) => &settling.said,
        Parsed::ForgetIssue(forget) => &forget.said,
        Parsed::Refused(refused) => &refused.said,
    }
}

/// `output` with the issue trigger's hint for the agent after its echoes,
/// and the fire as one more audit outcome. The user's message is unchanged.
fn with_hint(output: Output, fired: &Fired, session: &SessionId, prompt: &str) -> Output {
    let Output::Hook(mut reply) = output else {
        return output;
    };
    let session = session.to_string();
    let hint = scribe_core::issue_trigger::hint(fired, &session);
    reply.context = Some(match reply.context.take() {
        Some(context) => format!("{context}\n{hint}"),
        None => hint,
    });
    reply.outcomes.push(HookOutcome {
        target: Some(format!("session:{session}")),
        error_code: None,
        // Ids, the configured phrase and the prompt's hash: never its words.
        issue: Some(scribe_core::IssueAudit {
            verb: String::from(scribe_core::issue_trigger::FIRE_VERB),
            session: Some(session.clone()),
            trigger: Some(fired.trigger.clone()),
            request_sha256: vec![capture::prompt_sha256(prompt)],
            ..scribe_core::IssueAudit::default()
        }),
    });
    Output::Hook(reply)
}

/// The echoes of `prompt`'s annotation markers, as the user's text and the
/// agent's context with its note, for the Codex adapter.
pub(super) fn mark_echoes(prompt: &str) -> Vec<(String, String)> {
    marked(prompt)
        .into_iter()
        .map(|echo| {
            let context = format!("{} {}", echo.text, echo.note);
            (echo.text, context)
        })
        .collect()
}

/// The echoes of a prompt's annotation markers: one for those Stop will
/// catalog, and one per marker refused before any call. The refusals are
/// recorded in `marks.json` and audited at Stop, so none is audited here.
fn marked(prompt: &str) -> Vec<Echo> {
    let marks = annotation::marks(prompt);
    let mut echoes = Vec::new();
    match marks.iter().filter(|m| m.problem.is_none()).count() {
        0 => {}
        1 => echoes.push(Echo::marked(String::from(
            "scribe: marked — cataloged after the reply",
        ))),
        n => echoes.push(Echo::marked(format!(
            "scribe: marked {n} times — cataloged after the reply"
        ))),
    }
    for mark in &marks {
        if let Some(problem) = &mark.problem {
            echoes.push(Echo::not_recorded(format!(
                "scribe: refused #scribe in \"{}\" — {}",
                quote(&mark.said),
                problem.message()
            )));
        }
    }
    echoes
}

fn refusal(session: &SessionId, code: &'static str) -> HookOutcome {
    HookOutcome {
        target: Some(format!("session:{session}")),
        error_code: Some(code),
        issue: None,
    }
}

/// The reply: nothing at all when there is nothing to echo. The user sees
/// the plain echoes; the agent sees each echo with its own note, so a
/// recorded line never lends its "no action needed" to a refused one.
fn reply(echoes: Vec<Echo>, outcomes: Vec<HookOutcome>) -> Output {
    let (message, context) = if echoes.is_empty() {
        (None, None)
    } else {
        let text = echoes
            .iter()
            .map(|echo| echo.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let context = echoes
            .iter()
            .map(|echo| format!("{} {}", echo.text, echo.note))
            .collect::<Vec<_>>()
            .join("\n");
        (Some(text), Some(context))
    };
    Output::Hook(HookReply {
        event: EVENT,
        message,
        context,
        block: None,
        outcomes,
    })
}

/// `text` on one line, cut to [`ECHO_CHARS`].
pub(super) fn quote(text: &str) -> String {
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
pub(super) fn chain(err: &scribe_core::Error) -> String {
    let mut message = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}
