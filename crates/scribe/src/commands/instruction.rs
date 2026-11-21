//! `--session` and `--instruction`: the check that a write an agent runs on
//! Daniel's word is backed by a turn he typed (docs/design/scribe/2_design.md
//! §12.2, §12.5).
//!
//! The flags come together or not at all (a usage error, exit 2). With them,
//! [`check`] finds the session's transcript on this host, reads it through
//! the provenance check of §4.1 (one no-follow handle for check and read, as
//! the action-tag quote check does) and hands the bytes to
//! [`scribe_core::instruction`], before the command takes the store lock.
//! Any refusal changes nothing.

use super::hook::payload::{self, Unreadable};
use crate::app::Context;
use crate::error::CliError;
use clap::Args;
use scribe_core::instruction::{self, Act, Instructed, Request, Target};
use scribe_core::{SessionId, WriteRefusal};

/// The instruction a write is run on: given together or not at all.
#[derive(Debug, Args)]
pub(crate) struct InstructionArgs {
    /// The session you run in, whose transcript holds Daniel's instruction; requires --instruction
    #[arg(long, value_name = "ID", requires = "instruction")]
    pub(crate) session: Option<SessionId>,

    /// Daniel's instruction, word for word from one of his last three typed turns of --session within 30 minutes, naming the id (or answering your reply that names it) and the act; requires --session
    #[arg(long, value_name = "WORDS", requires = "session")]
    pub(crate) instruction: Option<String>,
}

/// Check the instruction `args` carry for `act` on `target`, now. `None`
/// when the command carries none.
///
/// # Errors
///
/// `instruction_not_found` when the session's transcript is not on this host
/// or fails the provenance check, and the refusals of
/// [`scribe_core::instruction::check`].
pub(super) fn check(
    ctx: &Context,
    args: &InstructionArgs,
    act: Act,
    target: &Target,
) -> Result<Option<Instructed>, CliError> {
    let (Some(session), Some(quote)) = (&args.session, &args.instruction) else {
        return Ok(None);
    };
    args.note(ctx);
    let not_found = |why: String| -> CliError {
        scribe_core::Error::AgentWrite {
            kind: WriteRefusal::InstructionNotFound,
            message: format!(
                "cannot check the instruction: {why}; run on the host where Daniel typed it and pass the session you run in"
            ),
        }
        .into()
    };
    let Some(path) = payload::locate(ctx, session)? else {
        return Err(not_found(format!(
            "no transcript of session {session} on this host"
        )));
    };
    let bytes = match payload::read_checked_bytes(ctx, &path, session) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return Err(not_found(format!(
                "no transcript of session {session} on this host"
            )));
        }
        Err(Unreadable::Refused(why)) => return Err(not_found(why)),
        Err(Unreadable::Io(source)) => {
            return Err(CliError::Read {
                what: path.display().to_string(),
                source,
            });
        }
    };
    let spoken = match ctx.provider() {
        scribe_core::Provider::ClaudeCode => instruction::claude_conversation(&bytes),
        scribe_core::Provider::Codex => instruction::codex_conversation(&bytes, session)?,
    };
    let request = Request {
        session,
        quote,
        act,
        target,
        now: time::OffsetDateTime::now_utc(),
    };
    let instructed = instruction::check(&spoken, &request)?;
    ctx.note_instruction_message(&instructed.message);
    Ok(Some(instructed))
}

impl InstructionArgs {
    /// Note `--session` for the command's audit line; call it first, so a
    /// refusal before the check is attributed too.
    pub(crate) fn note(&self, ctx: &Context) {
        if let Some(session) = &self.session {
            ctx.note_instruction_session(session.as_str());
        }
    }
}
