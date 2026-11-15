//! The `hook` noun: the entry points Claude Code's hooks run, each reading
//! the hook's JSON on stdin (docs/design/scribe/2_design.md §1, §4). One
//! file per event:
//!
//! - `user_prompt_submit.rs`: record each `#scribe` line of a typed prompt
//!   and reply with what was recorded or refused; a prompt whose first line
//!   is `/scribe on|off|status|link|catalog` goes to `scribe_command.rs`
//!   instead, which switches cataloging and blocks the prompt (§5.1, §5.8);
//! - `stop.rs`: after a reply, fill in the message uuid of the entries the
//!   prompt wrote, from the transcript, and mark a switched-on session's
//!   pass due by minutes or turns (§5.2);
//! - `session_end.rs`: mark a switched-on session's last pass due and final;
//! - `session_start.rs`: tell Daniel how many proposals are open (§7).
//!
//! A `#scribe approve|reject|retire|forget` line goes from
//! `user_prompt_submit.rs` to `proposal_command.rs`, which settles a
//! proposal or hides or blanks an entry (§3, §7).
//!
//! `payload.rs` holds the stdin shapes and the transcript provenance check;
//! `project.rs` finds the repository a session's cwd belongs to (both also
//! serve `session pass`). These are the only commands that write entries or
//! `control.json`, and the only ones that settle proposals; every one that
//! writes is audited (`session-start` reads only), and none calls a model
//! except the form-3 referent in `user_prompt_submit.rs`.

mod codex;
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
and nothing otherwise; never blocks such a prompt. #scribe approve <p-id>,
#scribe reject <p-id>, #scribe retire <entry-id> and #scribe forget
<entry-id> settle a proposal or hide or blank an entry. A prompt whose first
line is /scribe on, /scribe off or /scribe status switches this session's
cataloging and is blocked, its status given as the reason; /scribe catalog
<id> starts Daniel's catalog of a past session, whose windows the next
session pass runs.

Examples:
  scribe hook user-prompt-submit < <payload.json>";

const STOP_EXAMPLES: &str = "\
Reads the Stop payload (session_id, transcript_path) on stdin. Prints
nothing. Calls no model.

Examples:
  scribe hook stop < <payload.json>";

const SESSION_START_EXAMPLES: &str = "\
Reads the SessionStart payload (session_id) on stdin. Prints a message for
Daniel when proposals are open, and nothing otherwise. Reads only; calls no
model.

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
    /// Tell Daniel how many proposals are open, from the SessionStart hook's JSON on stdin
    #[command(after_help = SESSION_START_EXAMPLES)]
    SessionStart(HookArgs),
    /// Record each #scribe line of a typed prompt, or settle the proposal or entry it names, from the UserPromptSubmit hook's JSON on stdin
    #[command(after_help = PROMPT_EXAMPLES)]
    UserPromptSubmit(HookArgs),
    /// Fill in the message ids of entries this session recorded and mark a session pass due, from the Stop hook's JSON on stdin
    #[command(after_help = STOP_EXAMPLES)]
    Stop(HookArgs),
    /// Mark the final session pass due for a session that is on, from the SessionEnd hook's JSON on stdin
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
