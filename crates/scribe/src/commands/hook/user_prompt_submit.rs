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
    let mut output = capture_prompt(&payload, ctx)?;
    if ctx.provider() != Provider::ClaudeCode || payload::agent_originated(&payload) {
        return Ok(output);
    }
    let session = payload::session(&payload.session_id)?;
    if !ctx.store().take_deferred_ancestry(&session)? {
        return Ok(output);
    }
    let listed = ctx.store().list_sessions(&scribe_core::SessionFilter {
        include_inactive: true,
        ..Default::default()
    })?;
    let linked = listed.control.get(&session).is_some_and(|c| {
        c.pending_links
            .iter()
            .any(|l| l.kind == scribe_core::LinkType::Continues)
    }) || listed.listing.items.iter().any(|r| {
        r.id == session
            && r.links
                .iter()
                .any(|l| l.kind == scribe_core::LinkType::Continues)
    });
    let held = matches!(
        ctx.store().inherit_seat(&session, None)?,
        scribe_core::group::SeatStart::AlreadySeated(_) | scribe_core::group::SeatStart::Replaced
    );
    let predecessor =
        super::continuation::cleared_predecessor(ctx, Some(&payload.transcript_path), &session);
    let (restored, seat) = if !linked && !held {
        (
            super::continuation::at_start(ctx, &session, &predecessor),
            super::session_start::seat_at_start(ctx, &session, &predecessor),
        )
    } else {
        (None, Default::default())
    };
    if let Output::Hook(ref mut reply) = output {
        let mut contexts = Vec::new();
        if let Some(restored) = restored {
            contexts.extend(restored.context);
            if let Some(message) = restored.message {
                reply.message = Some(
                    [Some(message), reply.message.take()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            reply.outcomes.extend(restored.outcomes);
        }
        if let Some(message) = seat.message {
            reply.message = Some(
                [Some(message), reply.message.take()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        contexts.extend(seat.context);
        contexts.extend(reply.context.take());
        if !contexts.is_empty() {
            reply.context = Some(contexts.join("\n\n"));
        }
        reply.outcomes.extend(seat.outcomes);
        reply.outcomes.push(HookOutcome {
            target: Some(format!("session:{session}")),
            error_code: None,
        });
    }
    Ok(output)
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
    let first = payload.prompt.lines().next().unwrap_or("").trim();
    let words: Vec<_> = first.split_whitespace().collect();
    let group_command = matches!(words.as_slice(), ["/scribe" | "/scribe:scribe", "group", _]);
    if payload::agent_originated(payload) {
        if group_command
            || payload.prompt.lines().any(|line| {
                let parts: Vec<_> = line.split_whitespace().collect();
                matches!(parts.as_slice(), ["/scribe" | "/scribe:scribe", "group", _])
            })
        {
            return Ok(group_reply(
                "scribe: group refused — the prompt is not a typed user turn".into(),
                Some("group_membership"),
                &session,
            ));
        }
        return Ok(reply(Vec::new(), Vec::new()));
    }
    if group_command {
        if let Err(reason) = payload::check(ctx, &payload.transcript_path, &session) {
            return Ok(group_reply(
                format!("scribe: group refused — {reason}"),
                Some(PROVENANCE_FAILED),
                &session,
            ));
        }
        let project = project::of(std::path::Path::new(&payload.cwd));
        return Ok(
            match ctx
                .store()
                .join_project_group(words[2], &project, &session, ctx.now())
            {
                Ok(id) => group_reply(
                    format!("scribe: joined group {} ({id})", words[2]),
                    None,
                    &session,
                ),
                Err(err) => group_reply(
                    format!("scribe: group refused — {err}"),
                    Some(err.code()),
                    &session,
                ),
            },
        );
    }
    if let Some(switch) = scribe_core::control::parse(&payload.prompt) {
        return Ok(super::scribe_command::run(&switch, payload, &session, ctx));
    }
    let mut outcomes: Vec<HookOutcome> = if ctx.provider() == Provider::ClaudeCode {
        super::group_turns::claude(ctx, &session, &payload.transcript_path)?
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
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
        return Ok(reply(mark_echoes, outcomes));
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
        outcomes.extend(parsed.iter().map(|_| refusal(&session, PROVENANCE_FAILED)));
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
        }
    }
    echoes.extend(mark_echoes);
    Ok(reply(echoes, outcomes))
}

fn group_reply(reason: String, error_code: Option<&'static str>, session: &SessionId) -> Output {
    Output::Hook(HookReply {
        event: EVENT,
        message: None,
        context: None,
        block: Some(reason),
        outcomes: vec![HookOutcome {
            target: Some(format!("session:{session}")),
            error_code,
        }],
    })
}

/// The line a parsed settling command came from, as typed.
fn said_of(item: &Parsed) -> &str {
    match item {
        Parsed::Settle(settling) => &settling.said,
        Parsed::Refused(refused) => &refused.said,
    }
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
