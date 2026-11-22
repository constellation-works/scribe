//! The `hook` noun: the entry points Claude Code's hooks run, each reading
//! the hook's JSON on stdin (docs/design/scribe/2_design.md §1, §4). One
//! file per event:
//!
//! - `user_prompt_submit.rs`: echo the `#scribe` markers of a typed prompt
//!   and apply its settling commands; a prompt whose first line
//!   is `/scribe on|off|status|link|catalog` goes to `scribe_command.rs`
//!   instead, which switches cataloging and blocks the prompt (§5.1, §5.8);
//! - `stop.rs`: after a reply, catalog the `#scribe` markers Daniel typed
//!   through `annotation.rs` (§14), fill in message ids, and mark a
//!   switched-on session's pass due by minutes or turns (§5.2);
//! - `session_end.rs`: mark a switched-on session's last pass due and final;
//! - `session_start.rs`: tell Daniel how many proposals are open (§7),
//!   how many action entries no consumer has picked up (§4.6), and the
//!   `/scribe on --from <id>` command when this session may continue a
//!   recorded one (§5.4).
//!
//! A `#scribe approve|reject|retire|forget` line goes from
//! `user_prompt_submit.rs` to `proposal_command.rs`, which settles a
//! proposal or hides or blanks an entry (§3, §7); `#scribe forget
//! <issue-id>[#<n>]` goes to `issue_command.rs` (§11.4).
//!
//! `payload.rs` holds the stdin shapes and the transcript provenance check;
//! `project.rs` finds the repository a session's cwd belongs to (both also
//! serve `session pass`). These are the only commands that write entries or
//! `control.json`, and the only ones that settle proposals; every one that
//! writes is audited (`session-start` reads only), and none calls a model
//! except Stop's annotation call in `annotation.rs`.

mod annotation;
mod codex;
mod issue_command;
pub(super) mod payload;
pub(super) mod project;
mod proposal_command;
mod scribe_command;
mod session_end;
mod session_start;
mod stop;
mod user_prompt_submit;

use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::Output;
use clap::{Args, Subcommand};

pub(super) const HOOK_EXAMPLES: &str = "\
These run from provider packages or scribe init hooks, not by hand.
Use --provider codex for Codex; the default is claude-code.

Examples:
  scribe hook session-start < <payload.json>
  scribe hook user-prompt-submit < <payload.json>
  scribe hook stop < <payload.json>
  scribe hook session-end < <payload.json>";

const PROMPT_EXAMPLES: &str = "\
Reads the UserPromptSubmit payload (session_id, transcript_path, cwd, prompt)
on stdin. Prints Claude Code hook JSON when the prompt has a #scribe line,
and nothing otherwise; never blocks such a prompt. Every #scribe Daniel
typed, inside a sentence or opening a line (#scribe idea: ..., #scribe add
this to preferences), not quoted or in code, is a marker: it is echoed as
marked and cataloged at Stop, and nothing is written here; no model is
called. #scribe approve <p-id>, #scribe reject <p-id>, #scribe retire
<entry-id> and #scribe forget <entry-id>, each on its own line, settle a
proposal or hide or blank an entry; #scribe forget <issue-id> or
<issue-id>#<n> blanks an issue or removes an occurrence. A prompt whose first
line is /scribe on, /scribe off or /scribe status switches this session's
cataloging and is blocked, its status given as the reason; /scribe catalog
<id> starts Daniel's catalog of a past session, whose windows the next
session pass runs.

Examples:
  scribe hook user-prompt-submit < <payload.json>";

const STOP_EXAMPLES: &str = "\
Reads the Stop payload (session_id, transcript_path, optional cwd) on stdin.
Each typed turn with a #scribe marker that marks.json has not settled gets
one models.annotation call (30 s; at most 2 turns per Stop and 3 markers
per turn; a failed call is retried at the next reply, 3 attempts in all),
and its checked answer is recorded as Daniel's entry with scribe's statement
and his quoted words. A tag named after the marker, or at the end of a line
it opens, is a hint the model must follow. #scribe/global in place of
#scribe records the entry with project global, for every repository; the
working directory's project stays the default. Under an action tag
(standard, idea, or any tag config marks action = true) the entry also keeps
the turns since the previous #scribe turn, capped at 40 turns and 12000
characters and cut from the oldest. A line an older entry already records is
not cataloged again, and settling commands are never replayed. Prints hook
JSON saying what was recorded or why not; otherwise nothing.

Examples:
  scribe hook stop < <payload.json>";

const SESSION_START_EXAMPLES: &str = "\
Reads the SessionStart payload (session_id, and cwd when the harness sends
it) on stdin. Prints a message for Daniel when proposals are open, when
action entries have waited over an hour with no pickup, or when this
session is not opted in and a session in the same project was recorded
within the last day. Nothing otherwise, and nothing for the agent. Reads
only; calls no model.

Examples:
  scribe hook session-start < <payload.json>";

const SESSION_END_EXAMPLES: &str = "\
Reads the SessionEnd payload (session_id) on stdin. Prints nothing. Calls
no model; the pass itself runs from scribe session pass.

Examples:
  scribe hook session-end < <payload.json>";

/// Verbs on the `hook` noun: one per Claude Code hook event.
#[derive(Debug, Subcommand)]
pub(crate) enum HookCommand {
    /// Handle the SessionStart hook JSON from stdin
    ///
    /// Tells Daniel how many proposals are open, which action entries are unpicked, and when this session may continue a recorded one, from the SessionStart hook's JSON on stdin.
    #[command(after_help = SESSION_START_EXAMPLES)]
    SessionStart(HookArgs),
    /// Handle the UserPromptSubmit hook JSON from stdin
    ///
    /// Echoes each #scribe marker of a typed prompt, or settles the proposal or entry a settling line names, from the UserPromptSubmit hook's JSON on stdin.
    #[command(after_help = PROMPT_EXAMPLES)]
    UserPromptSubmit(HookArgs),
    /// Handle the Stop hook JSON from stdin
    ///
    /// Catalogs #scribe markers, fills in message ids and marks a session pass due, from the Stop hook's JSON on stdin.
    #[command(after_help = STOP_EXAMPLES)]
    Stop(HookArgs),
    /// Handle the SessionEnd hook JSON from stdin
    ///
    /// Marks the final session pass due for a session that is on, from the SessionEnd hook's JSON on stdin.
    #[command(after_help = SESSION_END_EXAMPLES)]
    SessionEnd(HookArgs),
}

/// Arguments to a hook verb: none, the payload is on stdin.
#[derive(Debug, Args)]
pub(crate) struct HookArgs {}

impl HookCommand {
    /// Every verb but `session-start` writes, so those are audited; a run
    /// that changed nothing writes no line (see `audit_middleware`).
    /// `session-start` reads only (STD-01 R31).
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::SessionStart(_) => None,
            Self::UserPromptSubmit(_) => Some(AuditMeta {
                command: "hook user-prompt-submit",
            }),
            Self::Stop(_) => Some(AuditMeta {
                command: "hook stop",
            }),
            Self::SessionEnd(_) => Some(AuditMeta {
                command: "hook session-end",
            }),
        }
    }
}

/// Run one `hook` verb on the payload read from stdin.
pub(super) fn run(command: HookCommand, ctx: &Context) -> Result<Output, CliError> {
    let input = payload::read_stdin()?;
    if ctx.provider() == scribe_core::Provider::Codex {
        let event = match command {
            HookCommand::SessionStart(_) => "SessionStart",
            HookCommand::UserPromptSubmit(_) => "UserPromptSubmit",
            HookCommand::Stop(_) => "Stop",
            HookCommand::SessionEnd(_) => "SessionEnd",
        };
        return codex::run(event, &input, ctx);
    }
    match command {
        HookCommand::SessionStart(HookArgs {}) => session_start::run(&input, ctx),
        HookCommand::UserPromptSubmit(HookArgs {}) => user_prompt_submit::run(&input, ctx),
        HookCommand::Stop(HookArgs {}) => stop::run(&input, ctx),
        HookCommand::SessionEnd(HookArgs {}) => session_end::run(&input, ctx),
    }
}

#[cfg(test)]
mod tests;
