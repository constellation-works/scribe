//! `scribe hook user-prompt-submit`: echo the `#scribe` markers of a typed
//! prompt and apply its settling commands (docs/design/scribe/2_design.md
//! §4.4, §14; ORB-27707 step 3b).
//!
//! A turn that is not plainly typed by the user (an agent or harness wrapper
//! such as `<agent-message>`, `<task-notification>`,
//! `<side-sessions-event>` or a scheduled/loop wrapper, or a non-plain
//! payload origin field; see the CLI's `payload::agent_originated`) is skipped at
//! once: it echoes nothing, settles nothing and does not run `/scribe`.
//!
//! Every `#scribe` the user typed, inside a sentence or opening a line
//! (`#scribe idea: …`, `#scribe add this to preferences`), is a marker
//! (§14.6): this hook writes nothing and calls no model for it, it only
//! echoes that the marker is noted; the Stop of this turn catalogs it
//! ([`crate::cases::annotation`]). Under Codex the adapter (the CLI's `codex.rs`) echoes them
//! instead, once it has verified the typed turn.
//!
//! A settling line (`#scribe approve|reject <p-id>`,
//! `#scribe retire|forget <entry-id>`, form 4) is a whole-line command, not
//! a marker: after the transcript check it goes to [`crate::cases::proposal_command`]
//! (or `issue_command.rs` for an issue id) and is echoed in its place.
//!
//! A prompt whose first line is `/scribe on`, `/scribe off` or
//! `/scribe status` is not a turn: [`crate::cases::scribe_command`] switches the session
//! and blocks it (§5.1), and its `#scribe` lines, if any, are not read.
//!
//! The reply tells the user (`systemMessage`) one echo per settling line and
//! one for the markers. The main agent (`additionalContext`) gets the same
//! echoes, each with a note: applied lines say no action is needed,
//! refused or failed lines say they must be relayed to the user, and markers
//! say Stop hands them to the agent after the reply, so the agent records
//! nothing for them before then. The hook never blocks the
//! prompt: a refusal or a store failure is a reply, not an error exit, so
//! the rest of the prompt still reaches the agent.

use crate::capture::{self, Parsed, Turn};
use crate::cases::Ctx;
use crate::cases::continuation;
use crate::cases::group::GroupVerbs;
use crate::cases::group_add::{self, GroupAddInput};
use crate::cases::group_turns;
use crate::cases::hook::{
    HookError, HookOutcome, HookPortError, HookReply, PROVENANCE_FAILED, chain, quote, refusal,
};
use crate::cases::proposal_command;
use crate::cases::scribe_command::{self, ScribeCommandInput};
use crate::cases::session::SessionVerbs;
use crate::cases::session_start;
use crate::message::NewMessage;
use crate::{Label, Provider, SessionId, annotation};
use std::path::Path;

/// What the prompt hook runs on: its parsed payload.
pub struct PromptInput<'i> {
    /// The session.
    pub session: &'i SessionId,
    /// Its transcript.
    pub transcript_path: &'i Path,
    /// The prompt's working directory.
    pub cwd: &'i str,
    /// The prompt as typed.
    pub prompt: &'i str,
    /// Whether the payload's origin claims say an agent or harness sent it.
    pub agent_originated: bool,
    /// The provider whose hook this is.
    pub provider: Provider,
    /// This host, as records name it.
    pub host: Label,
}

/// The hook event this replies to.
pub const EVENT: &str = "UserPromptSubmit";
/// What the agent is told after an applied echo, so it does not act on it
/// again.
const AGENT_NOTE: &str = "(recorded by scribe; no action needed)";
/// What the agent is told after a refused or failed echo: nothing was
/// applied, so the line must not be silently lost.
const AGENT_RELAY_NOTE: &str = "(not recorded by scribe; relay this to the user)";
/// What the agent is told after a marked echo: Stop hands the marker to the
/// agent after the reply, with the command that records it, so the agent
/// records nothing for it before then.
const AGENT_MARKED_NOTE: &str = "(after your reply scribe hands this #scribe marker to you with the command that records it; record nothing for it before then)";

/// Run the hook on its parsed payload, `read_past` reading a named
/// session's past transcript for `/scribe group add`.
///
/// # Errors
///
/// [`HookError`]: a store error, the transcript, a port, or a session of
/// another provider.
pub fn run<E: HookPortError>(
    ctx: &Ctx<'_, E>,
    input: &PromptInput<'_>,
    read_past: impl FnMut(Provider, &SessionId, &Path) -> Result<Option<Vec<NewMessage>>, E>,
) -> Result<HookReply, HookError<E>> {
    let mut output = capture_prompt(ctx, input, read_past)?;
    // A blocked reply (`decision: "block"`) drops context and message on render
    // and never reaches the agent, so keep any deferred ancestry check pending.
    if input.provider != Provider::ClaudeCode || input.agent_originated || output.block.is_some() {
        return Ok(output);
    }
    let session = input.session;
    // The echoes and writes above stand even when this step fails: the
    // failure is a reply, and the check stays pending for the next prompt.
    if let Err(err) = apply_deferred_ancestry(ctx, input, session, &mut output) {
        tracing::warn!(%err, "cannot apply the deferred ancestry check");
        ancestry_failure(session, &err, &mut output);
    }
    Ok(output)
}

fn apply_deferred_ancestry<E: HookPortError>(
    ctx: &Ctx<'_, E>,
    input: &PromptInput<'_>,
    session: &SessionId,
    output: &mut HookReply,
) -> Result<(), crate::Error> {
    if !ctx.store.has_deferred_ancestry(session)? {
        return Ok(());
    }
    let listed = ctx.store.read().sessions().list(&crate::SessionFilter {
        include_inactive: true,
        ..Default::default()
    })?;
    let linked =
        listed.control.get(session).is_some_and(|c| {
            c.pending_links
                .iter()
                .any(|l| l.kind == crate::LinkType::Continues)
        }) || listed.listing.items.iter().any(|r| {
            &r.id == session && r.links.iter().any(|l| l.kind == crate::LinkType::Continues)
        });
    let held = matches!(
        ctx.store.inherit_seat(session, None)?,
        crate::group::SeatStart::AlreadySeated(_) | crate::group::SeatStart::Replaced
    );
    if linked || held {
        if ctx.store.take_deferred_ancestry(session)? {
            output.outcomes.push(HookOutcome {
                target: Some(format!("session:{session}")),
                error_code: None,
            });
        }
        return Ok(());
    }
    // Keep the check pending while the transcript is still missing or unreadable.
    let Some(predecessor) =
        continuation::try_cleared_predecessor(ctx, input.provider, input.transcript_path, session)
    else {
        return Ok(());
    };
    if !ctx.store.take_deferred_ancestry(session)? {
        return Ok(());
    }
    let restored = continuation::at_start(ctx, session, &predecessor);
    let seat = session_start::seat_at_start(ctx, session, &predecessor);
    let reply = output;
    let mut contexts = Vec::new();
    if let Some(restored) = restored {
        contexts.extend(restored.context);
        if let Some(message) = restored.message {
            prepend_message(&mut reply.message, message);
        }
        reply.outcomes.extend(restored.outcomes);
    }
    if let Some(message) = seat.message {
        prepend_message(&mut reply.message, message);
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
    Ok(())
}

fn prepend_message(target: &mut Option<String>, message: String) {
    *target = Some(
        [Some(message), target.take()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

fn append_line(target: &mut Option<String>, line: String) {
    *target = Some(
        [target.take(), Some(line)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// The reply to a deferred ancestry check that failed, after the prompt's own
/// echoes. Consuming the marker is the check's only write, and it comes after
/// every call that can fail; restoration reports its own errors. So a failure
/// leaves the marker pending and nothing restored.
fn ancestry_failure(session: &SessionId, err: &crate::Error, reply: &mut HookReply) {
    let line = format!(
        "scribe: could not check this session's ancestry ({}); the check stays pending for the next prompt",
        chain(err)
    );
    append_line(&mut reply.message, line.clone());
    append_line(&mut reply.context, format!("{line} {AGENT_RELAY_NOTE}"));
    reply.outcomes.push(HookOutcome {
        target: Some(format!("session:{session}")),
        error_code: Some(err.code()),
    });
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

fn capture_prompt<E: HookPortError>(
    ctx: &Ctx<'_, E>,
    input: &PromptInput<'_>,
    read_past: impl FnMut(Provider, &SessionId, &Path) -> Result<Option<Vec<NewMessage>>, E>,
) -> Result<HookReply, HookError<E>> {
    let PromptInput {
        session,
        transcript_path,
        cwd,
        prompt,
        agent_originated,
        provider,
        ref host,
    } = *input;
    let session = session.clone();
    if ctx
        .store
        .read()
        .sessions()
        .control()?
        .get(&session)
        .is_some_and(|c| c.provider != provider)
    {
        return Err(HookError::Usage(String::from(
            "session belongs to another transcript provider",
        )));
    }
    if agent_originated {
        if prompt.lines().any(|line| group_line(line).is_some()) {
            return Ok(group_reply(
                "scribe: group refused — the prompt is not a typed user turn".into(),
                Some("group_membership"),
                &session,
            ));
        }
        return Ok(reply(Vec::new(), Vec::new()));
    }
    if let Some(line) = group_line(prompt).filter(|line| match line {
        GroupLine::Join(_) | GroupLine::Add { .. } => true,
        GroupLine::Move { .. } | GroupLine::Remove(_) | GroupLine::Update { .. } => {
            prompt.lines().count() == 1
        }
    }) {
        if let Err(reason) = ctx.transcripts.check(provider, transcript_path, &session) {
            return Ok(group_reply(
                format!("scribe: group refused — {reason}"),
                Some(PROVENANCE_FAILED),
                &session,
            ));
        }
        let (slug, named) = match line {
            GroupLine::Join(slug) => {
                let project = ctx.transcripts.project_of(Path::new(cwd));
                let redactor = redactor(ctx, provider)?;
                return Ok(
                    match ctx.store.join_project_group(
                        &slug,
                        &project,
                        &session,
                        ctx.clock.now(),
                        &redactor,
                    ) {
                        Ok(id) => group_reply(
                            format!("scribe: joined group {slug} ({id})"),
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
            GroupLine::Add { slug, sessions } => (slug, sessions),
            GroupLine::Move {
                session: named,
                slug,
            } => {
                let ids: Vec<String> = ctx
                    .store
                    .read()
                    .groups()
                    .all()?
                    .iter()
                    .flat_map(|g| g.members().into_iter().map(|id| id.to_string()))
                    .collect();
                let redactor = redactor(ctx, provider)?;
                let result = ctx
                    .store
                    .read()
                    .sessions()
                    .resolve_or_typed_with(&named, &ids)
                    .and_then(|id| {
                        let project = ctx.transcripts.project_of(Path::new(cwd));
                        ctx.store
                            .move_project_group(&id, &slug, &project, ctx.clock.now(), &redactor)
                            .map(|_| id)
                    });
                return Ok(named_reply(result, "moved", &session));
            }
            GroupLine::Remove(named) => {
                let ids: Vec<String> = ctx
                    .store
                    .read()
                    .groups()
                    .all()?
                    .iter()
                    .flat_map(|g| g.members().into_iter().map(|id| id.to_string()))
                    .collect();
                let result = ctx
                    .store
                    .read()
                    .sessions()
                    .resolve_or_typed_with(&named, &ids)
                    .and_then(|id| ctx.store.leave_group(&id).map(|_| id));
                return Ok(named_reply(result, "removed", &session));
            }
            GroupLine::Update {
                session: named,
                field,
                value,
            } => {
                let redactor = redactor(ctx, provider)?;
                let result = ctx.store.read().sessions().resolve(&named).and_then(|id| {
                    ctx.store
                        .update_session_metadata(&id, &field, &value, &redactor)
                        .map(|()| id)
                });
                return Ok(named_reply(result, "updated", &session));
            }
        };
        return group_add::run(
            ctx,
            &GroupAddInput {
                slug: &slug,
                named: &named,
                session: &session,
                cwd,
                provider,
            },
            read_past,
        );
    }
    if let Some(switch) = crate::control::parse(prompt).filter(|s| {
        !matches!(
            s,
            crate::control::Switch::GroupMove { .. }
                | crate::control::Switch::GroupRemove(_)
                | crate::control::Switch::SessionUpdate { .. }
        )
    }) {
        return Ok(scribe_command::run(
            ctx,
            &ScribeCommandInput {
                switch: &switch,
                session: &session,
                transcript_path,
                provider,
            },
        ));
    }
    let mut outcomes: Vec<HookOutcome> = if provider == Provider::ClaudeCode {
        group_turns::claude(ctx, provider, &session, transcript_path)?
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    // Markers (§14.6): echoed here, written nothing; the Stop of this turn
    // catalogs them. Under Codex the adapter echoes them once the typed
    // turn is verified (the CLI's `codex.rs`, [`mark_echoes`]), since its
    // Stop replays turns through here.
    let mark_echoes = if provider == Provider::Codex {
        Vec::new()
    } else {
        marked(prompt)
    };
    let parsed = capture::scan(prompt);
    if parsed.is_empty() {
        return Ok(reply(mark_echoes, outcomes));
    }

    // A missing transcript passes: the first prompt of a session can
    // arrive before Claude Code has written the file.
    if let Err(reason) = ctx.transcripts.check(provider, transcript_path, &session) {
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
        at: ctx.clock.now(),
        prompt_sha256: capture::prompt_sha256(prompt),
        cwd: cwd.to_owned(),
        project: ctx.transcripts.project_of(Path::new(cwd)),
        provider: Label::sanitized(provider.as_str(), provider.as_str()),
        host: host.clone(),
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
                let reply = proposal_command::run(ctx, &settling, &turn);
                echoes.push(Echo::settled(reply.echo, reply.outcome.as_ref()));
                outcomes.extend(reply.outcome);
            }
        }
    }
    echoes.extend(mark_echoes);
    Ok(reply(echoes, outcomes))
}

/// A typed project-group line.
enum GroupLine {
    Move {
        session: SessionId,
        slug: String,
    },
    Remove(SessionId),
    Update {
        session: SessionId,
        field: String,
        value: String,
    },
    /// `/scribe group <slug>`: the typing session joins.
    Join(String),
    /// `/scribe group add <slug> <session-id>...`: the named sessions join.
    Add {
        slug: String,
        sessions: Vec<SessionId>,
    },
}

/// The project-group line `text` opens with, read by the shared control
/// parser, so the subcommand is spelled as every control argument is:
/// case-insensitively. The slug is passed on as typed; the store checks it.
fn group_line(text: &str) -> Option<GroupLine> {
    match crate::control::parse(text) {
        Some(crate::control::Switch::Group(slug)) => Some(GroupLine::Join(slug)),
        Some(crate::control::Switch::GroupAdd { slug, sessions }) => {
            Some(GroupLine::Add { slug, sessions })
        }
        Some(crate::control::Switch::GroupMove { session, slug }) => {
            Some(GroupLine::Move { session, slug })
        }
        Some(crate::control::Switch::GroupRemove(session)) => Some(GroupLine::Remove(session)),
        Some(crate::control::Switch::SessionUpdate {
            session,
            field,
            value,
        }) => Some(GroupLine::Update {
            session,
            field,
            value,
        }),
        _ => None,
    }
}

fn named_reply(result: crate::Result<SessionId>, verb: &str, caller: &SessionId) -> HookReply {
    match result {
        Ok(id) => group_reply(format!("scribe: session {id} {verb}"), None, &id),
        Err(err) => group_reply(
            format!("scribe: {verb} refused — {err}"),
            Some(err.code()),
            caller,
        ),
    }
}

fn group_reply(reason: String, error_code: Option<&'static str>, session: &SessionId) -> HookReply {
    HookReply {
        event: EVENT,
        message: None,
        context: None,
        block: Some(reason),
        outcomes: vec![HookOutcome {
            target: Some(format!("session:{session}")),
            error_code,
        }],
    }
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
#[must_use]
pub fn mark_echoes(prompt: &str) -> Vec<(String, String)> {
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

/// The reply: nothing at all when there is nothing to echo. The user sees
/// the plain echoes; the agent sees each echo with its own note, so a
/// recorded line never lends its "no action needed" to a refused one.
fn reply(echoes: Vec<Echo>, outcomes: Vec<HookOutcome>) -> HookReply {
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
    HookReply {
        event: EVENT,
        message,
        context,
        block: None,
        outcomes,
    }
}

/// The redactor for the provider's config.
fn redactor<E: HookPortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
) -> Result<crate::Redactor, HookError<E>> {
    let config = ctx.store.read().tags().config()?.for_provider(provider);
    ctx.redaction.redactor(&config).map_err(HookError::Port)
}
