//! `--session` and `--instruction`: the check that a write an agent runs on
//! the user's word is backed by a turn they typed (docs/design/scribe/2_design.md
//! §12.2, §12.5).
//!
//! The flags come together or not at all (a usage error, exit 2). With them,
//! [`scribe_core::cases::instruction_check::check`] finds the session's
//! transcript on this host, reads it through the provenance check of §4.1
//! (one no-follow handle for check and read, as the action-tag quote check
//! does) and hands the bytes to [`scribe_core::instruction`], before the
//! command takes the store lock. Any refusal changes nothing.

use crate::app::Context;
use clap::Args;
use scribe_core::SessionId;
use scribe_core::cases::instruction_check::Instruction;

/// The instruction a write is run on: given together or not at all.
#[derive(Debug, Args)]
pub(crate) struct InstructionArgs {
    /// The session you run in, whose transcript holds the user's instruction (an id or a unique prefix of it); requires --instruction
    #[arg(long, value_name = "ID", requires = "instruction")]
    pub(crate) session: Option<SessionId>,

    /// The user's instruction, word for word from one of their last three typed turns of --session within 30 minutes, naming the id (or answering your reply that names it) and the act; requires --session
    #[arg(long, value_name = "WORDS", requires = "session")]
    pub(crate) instruction: Option<String>,
}

impl InstructionArgs {
    /// The flags as [`scribe_core::cases::instruction_check`] takes them.
    pub(crate) fn case<'a>(&'a self, ctx: &'a Context) -> Instruction<'a> {
        Instruction {
            session: self.session.as_ref(),
            quote: self.instruction.as_deref(),
            provider: ctx.provider(),
            notes: ctx,
        }
    }
}
