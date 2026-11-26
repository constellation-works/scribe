use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::group::{Command, GroupJoinInput, exec};
use scribe_core::composition::Context;
use scribe_core::{AuthorName, Label};

pub(super) const JOIN_EXAMPLES: &str =
    "Example: scribe group join <id> --as editor --author aide --role reviewer";

/// Arguments to `group join`.
#[derive(Debug, Args)]
pub(crate) struct JoinArgs {
    /// Group id, unique prefix, or project group slug
    id: String,
    /// Seat name (defaults to the short session id for a project group)
    #[arg(long = "as", value_name = "SEAT", value_parser = seat_name)]
    seat: Option<Label>,
    /// Your agent author name
    #[arg(long)]
    author: AuthorName,
    /// Role; defaults to member for a new project-group seat, otherwise the seat name.
    /// Omission on an existing seat keeps its current role.
    #[arg(long)]
    role: Option<String>,
    /// Explicitly replace the newest holder of an occupied seat
    #[arg(long)]
    replace: bool,
}

fn seat_name(value: &str) -> Result<Label, scribe_core::RecordError> {
    Label::try_from(value.to_owned())
}

pub(super) fn run(args: JoinArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Join(GroupJoinInput {
        session: ctx.caller(),
        id: args.id,
        seat: args.seat,
        author: args.author,
        role: args.role,
        replace: args.replace,
        provider: ctx.provider(),
    });
    Ok(output(exec(ctx, command)?))
}
