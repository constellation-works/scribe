//! `scribe hook user-prompt-submit`: record each `#scribe` line of a typed
//! prompt (docs/design/scribe/2_design.md §4.1–§4.4).
//!
//! A turn that is not plainly typed by Daniel (an agent or harness wrapper
//! such as `<agent-message>`, `<task-notification>`,
//! `<side-sessions-event>` or a scheduled/loop wrapper, or a non-plain
//! payload origin field; see `payload::agent_originated`) is skipped at
//! once: it records nothing, settles nothing and does not run `/scribe`.
//!
//! A prompt with no `#scribe` line returns at once: nothing is read, written,
//! audited or printed. Otherwise the transcript provenance is checked, the
//! config loaded, every line parsed, and the parsed ones recorded in one
//! store call, which finds an entry already written for the same line instead
//! of writing it again.
//!
//! A form-3 line (`#scribe add this to preferences`) is resolved first, outside
//! the store lock (§4.3): unless this prompt already recorded it, the
//! transcript's last 12 messages go to the referent model through the
//! `Models` port, and its span is checked verbatim; on a timeout, a failed
//! check or a model error the previous assistant message is recorded instead
//! and the echo says it was guessed. A session with no earlier message has
//! nothing to point at and the line is refused as `no_referent`.
//!
//! A line under an action tag (`#scribe standard: …`, `idea`
//! or any tag config marks `action = true`) also stores a transcript
//! window on its entry: the turns since the previous `#scribe` line of the
//! session, then the prompt, capped and cut from the oldest turn
//! (`capture::action_window`, §4.6). It reads the transcript once and calls
//! no model; other tags keep no window.
//!
//! A settling line (`#scribe approve|reject <p-id>`,
//! `#scribe retire|forget <entry-id>`, form 4) goes to `proposal_command.rs`
//! after the entries are recorded, and is echoed in its place.
//!
//! A prompt whose first line is `/scribe on`, `/scribe off` or
//! `/scribe status` is not a turn: `scribe_command.rs` switches the session
//! and blocks it (§5.1), and its `#scribe` lines, if any, are not read.
//!
//! The reply tells Daniel (`systemMessage`) one echo per `#scribe` line.
//! The main agent (`additionalContext`) gets the same echoes, each with a
//! note for that line: recorded lines say no action is needed, refused or
//! failed lines say they were not recorded and must be relayed to Daniel.
//! The hook never blocks the prompt: a refusal or a store failure is a
//! reply, not an error exit, so the rest of the prompt still reaches the
//! agent.

use super::payload::{self, PromptPayload, Transcript};
use super::project;
use crate::app::Context;
use crate::error::CliError;
use crate::output::{HookOutcome, HookReply, Output};
use scribe_core::capture::{self, Guess, Parsed, Refusal, Refused, Settling, Turn};
use scribe_core::models::ModelError;
use scribe_core::transcript::{self, Message};
use scribe_core::{Captured, Config, ContentFrom, Entry, Label, Models, SessionId};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The hook event this replies to.
const EVENT: &str = "UserPromptSubmit";
/// The longest quote an echo shows, in characters; `entry show` has it all.
const ECHO_CHARS: usize = 200;
/// What the agent is told after a recorded echo, so it does not record it
/// again.
const AGENT_NOTE: &str = "(recorded by scribe; no action needed)";
/// What the agent is told after a refused or failed echo: nothing was
/// recorded, so the line must not be silently lost.
const AGENT_RELAY_NOTE: &str = "(not recorded by scribe; relay this to Daniel)";
/// The code of a payload whose transcript failed the provenance check.
const PROVENANCE_FAILED: &str = "provenance_failed";

/// Run the hook on its stdin payload, resolving referents with the `claude`
/// CLI.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    run_with(input, ctx, |config| ctx.models(config))
}

/// One echo line, and whether its line was recorded (or already applied),
/// which decides the note the agent gets for it.
struct Echo {
    text: String,
    recorded: bool,
}

impl Echo {
    fn recorded(text: String) -> Self {
        Self {
            text,
            recorded: true,
        }
    }

    fn not_recorded(text: String) -> Self {
        Self {
            text,
            recorded: false,
        }
    }
}

/// What one `#scribe` line comes to before the store is called.
enum Plan {
    /// Refused while parsing or resolving.
    Refused(Refused),
    /// A draft goes to the store; `guess` says why a referent was guessed.
    Draft {
        line: u32,
        said: String,
        guess: Option<Guess>,
    },
    /// A referent line this prompt already recorded: echoed, the model not
    /// asked again.
    Recorded(Box<Entry>),
    /// A settling line (form 4), applied after the entries are recorded.
    Settle(Settling),
}

/// [`run`] with the `Models` adapter `models` builds for the config, so a
/// test can answer referents with a fake.
pub(super) fn run_with<M: Models>(
    input: &[u8],
    ctx: &Context,
    models: impl FnOnce(&Config) -> M,
) -> Result<Output, CliError> {
    let payload: PromptPayload = payload::decode(input)?;
    capture_prompt(&payload, ctx, models, None)
}

/// A transcript prompt's original position and already recorded lines.
pub(super) struct Recovery<'a> {
    pub(super) message: &'a Message,
    pub(super) before: &'a [Message],
    pub(super) recorded: BTreeSet<u32>,
}

/// Recover an entry-producing prompt without calling a model or replaying
/// session controls or settling commands. Referents use the existing guessed
/// fallback, with the same echo explaining why.
pub(super) fn recover(
    payload: &PromptPayload,
    ctx: &Context,
    recovery: Recovery<'_>,
) -> Result<Output, CliError> {
    capture_prompt(payload, ctx, |config| ctx.models(config), Some(recovery))
}

fn capture_prompt<M: Models>(
    payload: &PromptPayload,
    ctx: &Context,
    models: impl FnOnce(&Config) -> M,
    recovery: Option<Recovery<'_>>,
) -> Result<Output, CliError> {
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
        if recovery.is_some() {
            return Ok(reply(Vec::new(), Vec::new()));
        }
        return Ok(super::scribe_command::run(&switch, payload, &session, ctx));
    }
    let lines: Vec<_> = capture::lines(&payload.prompt)
        .into_iter()
        .filter(|(line, _)| !recovery.as_ref().is_some_and(|r| r.recorded.contains(line)))
        .collect();
    if lines.is_empty() {
        return Ok(reply(Vec::new(), Vec::new()));
    }
    let failed_all = |message: &str, code: &'static str| {
        let echoes = lines
            .iter()
            .map(|(_, said)| {
                Echo::not_recorded(format!(
                    "scribe: could not record \"{}\" — {message}",
                    quote(said)
                ))
            })
            .collect();
        let outcomes = lines.iter().map(|_| refusal(&session, code)).collect();
        Ok(reply(echoes, outcomes))
    };

    // A missing transcript passes: the first prompt of a session can
    // arrive before Claude Code has written the file.
    let transcript = match payload::check(ctx, &payload.transcript_path, &session) {
        Ok(found) => found,
        Err(reason) => {
            return failed_all(
                &format!("the hook payload failed the transcript check: {reason}"),
                PROVENANCE_FAILED,
            );
        }
    };
    let config = match ctx.config() {
        Ok(config) => config,
        Err(err) => return failed_all(&err.to_string(), err.code()),
    };
    // Fail closed before recording anything when a configured [redact] env
    // name has no value here: the capture would store it unmasked.
    let redactor = match ctx.redactor(&config) {
        Ok(redactor) => redactor,
        Err(err) => return failed_all(&err.to_string(), err.code()),
    };

    let at = recovery
        .as_ref()
        .map_or_else(|| ctx.now(), |r| r.message.at);
    let turn = Turn {
        session: session.clone(),
        at,
        prompt_sha256: capture::prompt_sha256(&payload.prompt),
        cwd: payload.cwd.clone(),
        // The cwd's project. A line that opens with `#scribe/global`
        // overrides this when its draft is built.
        project: project::of(std::path::Path::new(&payload.cwd)),
        provider: Label::sanitized(ctx.provider().as_str(), ctx.provider().as_str()),
        host: ctx.host(),
    };
    let parsed = capture::scan(&payload.prompt, &config.tags);
    let mut referents = Referents::new(ctx, &turn, transcript, &payload.transcript_path);
    if let Some(recovery) = &recovery {
        referents.messages = Some(recovery.before.to_vec());
    }
    let mut models = Some(models);
    let mut adapter: Option<M> = None;
    let mut drafts = Vec::new();
    let mut draft_lines = Vec::new();
    let mut plans = Vec::new();
    for item in parsed {
        if recovery
            .as_ref()
            .is_some_and(|r| r.recorded.contains(&item.line()))
        {
            continue;
        }
        let plan = match item {
            Parsed::Refused(refused) => Plan::Refused(refused),
            Parsed::Settle(settling) => {
                if recovery.is_some() {
                    continue;
                }
                Plan::Settle(settling)
            }
            Parsed::Instruction(instruction) => {
                let mut draft = capture::draft(&instruction, &turn, &config.tags);
                if config.tags.action(&draft.tag).is_some() {
                    draft.window = Some(capture::action_window(
                        referents.messages(),
                        &turn.prompt_sha256,
                        &payload.prompt,
                        turn.at,
                    ));
                }
                draft_lines.push(instruction.line);
                drafts.push(draft);
                Plan::Draft {
                    line: instruction.line,
                    said: instruction.said,
                    guess: None,
                }
            }
            Parsed::Referent(referent) => {
                if let Some(entry) = referents.recorded(referent.line) {
                    Plan::Recorded(Box::new(entry))
                } else {
                    let window = capture::window(referents.messages(), &turn.prompt_sha256);
                    if window.is_empty() {
                        let marked = referent.global;
                        Plan::Refused(Refused {
                            line: referent.line,
                            said: referent.said,
                            reason: Refusal::NoReferent,
                            marked,
                        })
                    } else {
                        if recovery.is_none() && adapter.is_none() {
                            adapter = models.take().map(|build| build(&config));
                        }
                        let request =
                            capture::referent_request(&referent, &window, &config.models.referent);
                        let answer = match &adapter {
                            Some(adapter) => adapter.referent(&request),
                            None => Err(ModelError::Unavailable(String::from(
                                "Stop recovery does not call models",
                            ))),
                        };
                        match capture::resolve(&referent, &window, answer, &turn, &config.tags) {
                            // Stop recovery skips a resolved referent that an
                            // entry covered when it was typed, as it skips a
                            // covered form-2 line before this runs: capture
                            // consults only entries active now, so a later
                            // retirement would otherwise write it. A read
                            // failure leaves the decision to capture.
                            Some((draft, _))
                                if recovery.is_some()
                                    && ctx
                                        .store()
                                        .covered_draft(&draft, turn.at, &redactor)
                                        .unwrap_or(false) =>
                            {
                                continue;
                            }
                            Some((mut draft, guess)) => {
                                if config.tags.action(&draft.tag).is_some() {
                                    draft.window = Some(capture::action_window(
                                        referents.messages(),
                                        &turn.prompt_sha256,
                                        &payload.prompt,
                                        turn.at,
                                    ));
                                }
                                draft_lines.push(referent.line);
                                drafts.push(draft);
                                Plan::Draft {
                                    line: referent.line,
                                    said: referent.said,
                                    guess,
                                }
                            }
                            None => {
                                let marked = referent.global;
                                Plan::Refused(Refused {
                                    line: referent.line,
                                    said: referent.said,
                                    reason: Refusal::NoReferent,
                                    marked,
                                })
                            }
                        }
                    }
                }
            }
        };
        plans.push(plan);
    }
    if let Some(recovery) = &recovery {
        for draft in &mut drafts {
            draft.cite.message = Some(recovery.message.uuid.clone());
        }
    }
    let stored = ctx.store().capture(drafts, &redactor);

    let mut captured: BTreeMap<u32, Captured> = BTreeMap::new();
    let mut store_error: Option<scribe_core::Error> = None;
    match stored {
        Ok(results) => {
            // Key by the draft's line. A covered statement's existing entry
            // cites a different line, so the new line would otherwise miss.
            for (line, result) in draft_lines.into_iter().zip(results) {
                captured.insert(line, result);
            }
        }
        Err(err) => store_error = Some(err),
    }

    let mut echoes = Vec::new();
    let mut outcomes = Vec::new();
    for plan in plans {
        match plan {
            Plan::Refused(refused) => {
                echoes.push(Echo::not_recorded(format!(
                    "scribe: refused \"{}\" — {}",
                    quote(&refused.said),
                    refused.message()
                )));
                outcomes.push(refusal(&session, refused.reason.code()));
            }
            Plan::Recorded(entry) => echoes.push(Echo::recorded(already(&entry))),
            Plan::Settle(settling) => {
                let reply = super::proposal_command::run(&settling, &turn, ctx);
                // An outcome with a code is a refusal or failure; `None`
                // (already applied) and a codeless outcome need no relay.
                let recorded = match &reply.outcome {
                    Some(outcome) => outcome.error_code.is_none(),
                    None => true,
                };
                echoes.push(if recorded {
                    Echo::recorded(reply.echo)
                } else {
                    Echo::not_recorded(reply.echo)
                });
                outcomes.extend(reply.outcome);
            }
            Plan::Draft { line, said, guess } => match (captured.get(&line), &store_error) {
                (Some(Captured::Added(entry)), _) => {
                    let guessed = guess
                        .map(|g| {
                            format!(
                                " (guessed: {g}; recorded the {} message before your prompt instead)",
                                previous(entry)
                            )
                        })
                        .unwrap_or_default();
                    echoes.push(Echo::recorded(format!(
                        "scribe: added {} to {}{guessed} — \"{}\"",
                        entry.id,
                        entry.tag,
                        quote(&entry.content)
                    )));
                    outcomes.push(HookOutcome {
                        target: Some(entry.id.to_string()),
                        error_code: None,
                    });
                }
                (Some(Captured::Existing(entry)), _) => echoes.push(Echo::recorded(already(entry))),
                (Some(Captured::Covered(entry)), _) => {
                    // Stop recovery must not repeat a covered line on later
                    // turns. The prompt hook (no recovery) echoes it once.
                    if recovery.is_none() {
                        echoes.push(Echo::recorded(covered(entry)));
                    }
                }
                (None, Some(err)) => {
                    echoes.push(Echo::not_recorded(format!(
                        "scribe: could not record \"{}\" — {}",
                        quote(&said),
                        chain(err)
                    )));
                    outcomes.push(refusal(&session, err.code()));
                }
                (None, None) => {}
            },
        }
    }
    Ok(reply(echoes, outcomes))
}

/// Whose message a guessed entry recorded, for its echo.
fn previous(entry: &Entry) -> &'static str {
    match &entry.content_from {
        ContentFrom::Message { role, .. } => role.as_str(),
        ContentFrom::Instruction => "last",
        ContentFrom::Agent => "agent",
    }
}

fn already(entry: &Entry) -> String {
    format!(
        "scribe: already recorded {} in {} — \"{}\"",
        entry.id,
        entry.tag,
        quote(&entry.content)
    )
}

fn covered(entry: &Entry) -> String {
    format!(
        "scribe: already covered by {} in {} — \"{}\"",
        entry.id,
        entry.tag,
        quote(&entry.content)
    )
}

/// What form-3 lines need, read at most once per run and only when a
/// referent line is present: the entries this prompt already recorded and
/// the transcript's messages.
struct Referents<'a> {
    ctx: &'a Context,
    turn: &'a Turn,
    transcript: Transcript,
    path: &'a Path,
    recorded: Option<Vec<Entry>>,
    messages: Option<Vec<Message>>,
}

impl<'a> Referents<'a> {
    fn new(ctx: &'a Context, turn: &'a Turn, transcript: Transcript, path: &'a Path) -> Self {
        Self {
            ctx,
            turn,
            transcript,
            path,
            recorded: None,
            messages: None,
        }
    }

    /// The entry this prompt already recorded for `line`. A store that
    /// cannot be read finds none; the capture that follows reports it.
    fn recorded(&mut self, line: u32) -> Option<Entry> {
        let (ctx, turn) = (self.ctx, self.turn);
        self.recorded
            .get_or_insert_with(|| {
                ctx.store()
                    .entries_from_prompt(&turn.session, &turn.prompt_sha256)
                    .unwrap_or_default()
            })
            .iter()
            .find(|e| e.cite.as_ref().is_some_and(|c| c.line == line))
            .cloned()
    }

    /// The transcript's messages: none when it does not exist yet or
    /// cannot be read.
    fn messages(&mut self) -> &[Message] {
        let (transcript, path) = (&self.transcript, self.path);
        self.messages.get_or_insert_with(|| match transcript {
            Transcript::Missing => Vec::new(),
            Transcript::Present => match payload::read_for(self.ctx, path) {
                Ok(bytes) => transcript::parse(&bytes),
                Err(err) => {
                    tracing::warn!(path = %path.display(), %err, "cannot read the transcript");
                    Vec::new()
                }
            },
        })
    }
}

fn refusal(session: &SessionId, code: &'static str) -> HookOutcome {
    HookOutcome {
        target: Some(format!("session:{session}")),
        error_code: Some(code),
    }
}

/// The reply: nothing at all when there is nothing to echo. Daniel sees
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
            .map(|echo| {
                let note = if echo.recorded {
                    AGENT_NOTE
                } else {
                    AGENT_RELAY_NOTE
                };
                format!("{} {note}", echo.text)
            })
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
