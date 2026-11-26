use super::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::entry::{Command, exec};
use scribe_core::composition::Context;

pub(super) const FORGET_EXAMPLES: &str = "\
Blanks the entry's text, evidence and reason for good, with every proposal
quote of it and every reason about it; the id, tag, dates and author stay,
and no file is removed. It cannot be undone, so it runs only on the user's
instruction (their own turn says forget, erase, purge, permanently or for good)
and with --confirm.

Examples:
  scribe entry forget <id> --author opus --session <id> --instruction '<their words>' --confirm";

#[derive(Debug, Args)]
pub(crate) struct ForgetArgs {
    /// Entry id or unique prefix (at least four characters)
    id: String,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Confirm blanking the entry's text for good; it cannot be undone
    #[arg(long)]
    confirm: bool,
    #[command(flatten)]
    instruction: super::super::instruction::InstructionArgs,
}

pub(super) fn run(args: ForgetArgs, ctx: &Context) -> Result<Output, CliError> {
    let ForgetArgs {
        id,
        author,
        confirm,
        instruction,
    } = args;
    let command = Command::Forget {
        id,
        author,
        confirm,
        instruction: instruction.case(ctx),
    };
    Ok(output::output(exec(ctx, command)?, false, None, ctx))
}
