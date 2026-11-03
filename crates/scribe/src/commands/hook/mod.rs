//! The `hook` noun: the entry points Claude Code's hooks run, each reading
//! the hook's JSON on stdin (docs/design/scribe/2_design.md §1, §4). One
//! file per event:
//!
//! - `user_prompt_submit.rs`: record each `#scribe` line of a typed prompt
//!   and reply with what was recorded or refused; a prompt whose first line
//!   is `/scribe on|off|status` goes to `scribe_command.rs` instead, which
//!   switches cataloging and blocks the prompt (§5.1);
//! - `stop.rs`: after a reply, fill in the message uuid of the entries the
//!   prompt wrote, from the transcript, and mark a switched-on session's
//!   pass due by minutes or turns (§5.2);
//! - `session_end.rs`: mark a switched-on session's last pass due and final.
//!
//! `payload.rs` holds the stdin shapes and the transcript provenance check;
//! `project.rs` finds the repository a session's cwd belongs to (both also
//! serve `session pass`). These are the only commands that write entries or
//! `control.json`; every one is audited, and none calls a model except the
//! form-3 referent in `user_prompt_submit.rs`.

pub(super) mod payload;
pub(super) mod project;
mod scribe_command;
mod session_end;
mod stop;
mod user_prompt_submit;

use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::Output;
use clap::{Args, Subcommand};

pub(super) const HOOK_EXAMPLES: &str = "\
These run from the Claude Code package (plugin/claude-code), not by hand.

Examples:
  scribe hook user-prompt-submit < <payload.json>
  scribe hook stop < <payload.json>
  scribe hook session-end < <payload.json>";

const PROMPT_EXAMPLES: &str = "\
Reads the UserPromptSubmit payload (session_id, transcript_path, cwd, prompt)
on stdin. Prints Claude Code hook JSON when the prompt has a #scribe line,
and nothing otherwise; never blocks such a prompt. A prompt whose first
line is /scribe on, /scribe off or /scribe status switches this session's
cataloging and is blocked, its status given as the reason.

Examples:
  scribe hook user-prompt-submit < <payload.json>";

const STOP_EXAMPLES: &str = "\
Reads the Stop payload (session_id, transcript_path) on stdin. Prints
nothing. Calls no model.

Examples:
  scribe hook stop < <payload.json>";

const SESSION_END_EXAMPLES: &str = "\
Reads the SessionEnd payload (session_id) on stdin. Prints nothing. Calls
no model; the pass itself runs from scribe session pass.

Examples:
  scribe hook session-end < <payload.json>";

/// Verbs on the `hook` noun: one per Claude Code hook event.
#[derive(Debug, Subcommand)]
pub(crate) enum HookCommand {
    /// Record each #scribe line of a typed prompt, from the UserPromptSubmit hook's JSON on stdin
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
    /// Every verb writes, so every verb is audited; a run that changed nothing
    /// writes no line (see `audit_middleware`).
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
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
    match command {
        HookCommand::UserPromptSubmit(HookArgs {}) => user_prompt_submit::run(&input, ctx),
        HookCommand::Stop(HookArgs {}) => stop::run(&input, ctx),
        HookCommand::SessionEnd(HookArgs {}) => session_end::run(&input, ctx),
    }
}

#[cfg(test)]
mod tests;
