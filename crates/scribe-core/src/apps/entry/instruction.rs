//! `--session` and `--instruction` (ORB-27707 step 3d): the check that a
//! write an agent runs on the user's word is backed by a turn they typed
//! (docs/design/scribe/2_design.md §12.2, §12.5).
//!
//! [`check`] finds the session's transcript on this host, reads it through
//! the provenance check of §4.1 (one no-follow handle for check and read)
//! and hands the bytes to [`crate::instruction`], before the command takes
//! the store lock. Any refusal changes nothing.

use crate::instruction::{self, Act, Instructed, Request, Target};
use crate::runtime::errors::WriteError;
use crate::runtime::{AuditNotes, Ctx, PortError, Unreadable, resolve};
use crate::{Provider, SessionId, WriteRefusal};

/// The instruction a write is run on, as the flags carry it.
#[derive(Clone, Copy)]
pub struct Instruction<'i> {
    /// `--session`: the session the user typed in, an id or a unique prefix.
    pub session: Option<&'i SessionId>,
    /// `--instruction`: the user's words.
    pub quote: Option<&'i str>,
    /// The provider whose transcripts hold the session.
    pub provider: Provider,
    /// Where the audit line's instruction fields are noted.
    pub notes: &'i dyn AuditNotes,
}

/// Note `--session` for the command's audit line, resolved across the store
/// and the transcripts on disk, or as typed when it does not resolve. Call
/// it first, so a refusal before the check is attributed too.
pub fn note<E: PortError>(ctx: &Ctx<'_, E>, instruction: &Instruction<'_>) {
    if let Some(session) = instruction.session {
        let session = resolve::session_or_typed(ctx, instruction.provider, session)
            .unwrap_or_else(|_| session.clone());
        instruction.notes.session(session.as_str());
    }
}

/// Check `instruction` for `act` on `target`, now. `None` when the command
/// carries none.
///
/// # Errors
///
/// `instruction_not_found` when the session's transcript is not on this host
/// or fails the provenance check, [`WriteError::Read`] when it cannot be
/// read, and the refusals of [`crate::instruction::check`].
pub fn check<E: PortError>(
    ctx: &Ctx<'_, E>,
    instruction: &Instruction<'_>,
    act: Act,
    target: &Target,
) -> Result<Option<Instructed>, WriteError<E>> {
    let (Some(typed), Some(quote)) = (instruction.session, instruction.quote) else {
        return Ok(None);
    };
    note(ctx, instruction);
    let provider = instruction.provider;
    // A unique prefix names the stored or uncataloged session; the check runs on the full id.
    let resolved = resolve::session_or_typed(ctx, provider, typed)?;
    let session = &resolved;
    let not_found = |why: String| -> WriteError<E> {
        WriteError::Core(crate::Error::AgentWrite {
            kind: WriteRefusal::InstructionNotFound,
            message: format!(
                "cannot check the instruction: {why}; run on the host where the user typed it and pass the session you run in"
            ),
        })
    };
    let path = ctx
        .transcripts
        .files(provider)
        .map_err(WriteError::Port)?
        .into_iter()
        .find(|(found, _)| found == session)
        .map(|(_, path)| path);
    let Some(path) = path else {
        return Err(not_found(format!(
            "no transcript of session {session} on this host"
        )));
    };
    let bytes = match ctx
        .transcripts
        .read_checked_unparsed(provider, &path, session)
    {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return Err(not_found(format!(
                "no transcript of session {session} on this host"
            )));
        }
        Err(Unreadable::Refused(why)) => return Err(not_found(why)),
        Err(Unreadable::Io(source)) => {
            return Err(WriteError::Read {
                what: path.display().to_string(),
                source,
            });
        }
    };
    let spoken = match provider {
        Provider::ClaudeCode => instruction::claude_conversation(&bytes),
        Provider::Codex => instruction::codex_conversation(&bytes, session)?,
    };
    // The turn is identified once found, whether or not it then passes:
    // a refusal's audit line names it too.
    if let Some(message) = instruction::located_message(&spoken, quote) {
        instruction.notes.message(message);
    }
    let request = Request {
        session,
        quote,
        act,
        target,
        now: ctx.clock.now_precise(),
    };
    Ok(Some(instruction::check(&spoken, &request)?))
}
