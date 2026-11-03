//! `scribe hook user-prompt-submit`: record each `#scribe` line of a typed
//! prompt (docs/design/scribe/2_design.md §4.1–§4.4).
//!
//! A prompt with no `#scribe` line returns at once: nothing is read, written,
//! audited or printed. Otherwise the transcript provenance is checked, the
//! config loaded, every line parsed, and the parsed ones recorded in one
//! store call, which finds an entry already written for the same line instead
//! of writing it again.
//!
//! A form-3 line (`#scribe add this to decisions`) is resolved first, outside
//! the store lock (§4.3): unless this prompt already recorded it, the
//! transcript's last 12 messages go to the referent model through the
//! `Models` port, and its span is checked verbatim; on a timeout, a failed
//! check or a model error the previous assistant message is recorded instead
//! and the echo says it was guessed. A session with no earlier message has
//! nothing to point at and the line is refused as `no_referent`.
//!
//! The reply tells Daniel (`systemMessage`) and the main agent
//! (`additionalContext`) the same thing, one line per `#scribe` line,
//! and never blocks the prompt: a refusal or a store failure is a reply, not
//! an error exit, so the rest of the prompt still reaches the agent.

use super::payload::{self, PromptPayload, Transcript};
use super::project;
use crate::app::Context;
use crate::error::CliError;
use crate::output::{HookOutcome, HookReply, Output};
use scribe_core::capture::{self, Guess, Parsed, Refusal, Refused, Turn};
use scribe_core::models::ModelError;
use scribe_core::transcript::{self, Message};
use scribe_core::{Captured, Config, ContentFrom, Entry, Label, Models, SessionId};
use std::collections::BTreeMap;
use std::path::Path;

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

/// Run the hook on its stdin payload, resolving referents with the `claude`
/// CLI.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    run_with(input, ctx, |config| ctx.models(config))
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
}

/// [`run`] with the `Models` adapter `models` builds for the config, so a
/// test can answer referents with a fake.
pub(super) fn run_with<M: Models>(
    input: &[u8],
    ctx: &Context,
    models: impl FnOnce(&Config) -> M,
) -> Result<Output, CliError> {
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
    let transcript =
        match payload::check_transcript(&payload.transcript_path, &session, ctx.transcripts()) {
            Ok(found) => found,
            Err(reason) => {
                return failed_all(
                    &format!("the hook payload failed the transcript check: {reason}"),
                    PROVENANCE_FAILED,
                );
            }
        };
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
    let mut referents = Referents::new(ctx, &turn, transcript, &payload.transcript_path);
    let mut models = Some(models);
    let mut adapter: Option<M> = None;
    let mut drafts = Vec::new();
    let mut plans = Vec::new();
    for item in parsed {
        let plan = match item {
            Parsed::Refused(refused) => Plan::Refused(refused),
            Parsed::Instruction(instruction) => {
                drafts.push(capture::draft(&instruction, &turn, &config.tags));
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
                        Plan::Refused(Refused {
                            line: referent.line,
                            said: referent.said,
                            reason: Refusal::NoReferent,
                        })
                    } else {
                        if adapter.is_none() {
                            adapter = models.take().map(|build| build(&config));
                        }
                        let request =
                            capture::referent_request(&referent, &window, &config.models.referent);
                        let answer = match &adapter {
                            Some(adapter) => adapter.referent(&request),
                            None => Err(ModelError::Unavailable(String::from("no model adapter"))),
                        };
                        match capture::resolve(&referent, &window, answer, &turn, &config.tags) {
                            Some((draft, guess)) => {
                                drafts.push(draft);
                                Plan::Draft {
                                    line: referent.line,
                                    said: referent.said,
                                    guess,
                                }
                            }
                            None => Plan::Refused(Refused {
                                line: referent.line,
                                said: referent.said,
                                reason: Refusal::NoReferent,
                            }),
                        }
                    }
                }
            }
        };
        plans.push(plan);
    }
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
    for plan in plans {
        match plan {
            Plan::Refused(refused) => {
                echoes.push(format!(
                    "scribe: refused \"{}\" — {}",
                    quote(&refused.said),
                    refused.reason
                ));
                outcomes.push(refusal(&session, refused.reason.code()));
            }
            Plan::Recorded(entry) => echoes.push(already(&entry)),
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
                    echoes.push(format!(
                        "scribe: added {} to {}{guessed} — \"{}\"",
                        entry.id,
                        entry.tag,
                        quote(&entry.content)
                    ));
                    outcomes.push(HookOutcome {
                        target: Some(entry.id.to_string()),
                        error_code: None,
                    });
                }
                (Some(Captured::Existing(entry)), _) => echoes.push(already(entry)),
                (None, Some(err)) => {
                    echoes.push(format!(
                        "scribe: could not record \"{}\" — {}",
                        quote(&said),
                        chain(err)
                    ));
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
            .find(|e| e.cite.line == line)
            .cloned()
    }

    /// The transcript's messages: none when it does not exist yet or
    /// cannot be read.
    fn messages(&mut self) -> &[Message] {
        let (transcript, path) = (&self.transcript, self.path);
        self.messages.get_or_insert_with(|| match transcript {
            Transcript::Missing => Vec::new(),
            Transcript::Present => match std::fs::read(path) {
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
