use super::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::entry::{Command, exec};
use scribe_core::composition::Context;

pub(super) const RETIRE_EXAMPLES: &str = "\
The user's entry retires only on their instruction: --session and --instruction.

Examples:
  scribe entry retire <id> --author opus --reason 'No longer applies'
  scribe entry retire <id> --author opus --reason 'The user asked' --session <id> --instruction '<their words>'";

#[derive(Debug, Args)]
pub(crate) struct RetireArgs {
    /// Entry id or unique prefix
    id: String,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Why the note no longer applies
    #[arg(long)]
    reason: String,
    #[command(flatten)]
    instruction: crate::commands::instruction::InstructionArgs,
}

pub(super) fn run(args: RetireArgs, ctx: &Context) -> Result<Output, CliError> {
    let RetireArgs {
        id,
        author,
        reason,
        instruction,
    } = args;
    let command = Command::Retire {
        id,
        reason,
        author,
        instruction: instruction.case(ctx),
    };
    Ok(output::output(exec(ctx, command)?, false, None, ctx))
}
