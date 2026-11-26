use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::group::{Command, GroupOpenInput, exec};
use scribe_core::composition::Context;
use scribe_core::{Project, Related};

pub(super) const OPEN_EXAMPLES: &str = "Example: scribe group open --title 'Release review' --kind review --quote 'open a release review'";

/// Arguments to `group open`.
#[derive(Debug, Args)]
pub(crate) struct OpenArgs {
    /// Title of the work
    #[arg(long)]
    title: String,
    /// Free label for the kind of work
    #[arg(long)]
    kind: String,
    /// Project slug
    #[arg(long, default_value = "global")]
    project: Project,
    /// Task, PR or repository reference, repeatable
    #[arg(long)]
    related: Vec<Related>,
    /// Words the user typed in this session
    #[arg(long)]
    quote: Option<String>,
}

pub(super) fn run(args: OpenArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Open(GroupOpenInput {
        session: ctx.caller(),
        title: args.title,
        kind: args.kind,
        project: args.project,
        related: args.related,
        quote: args.quote,
        provider: ctx.provider(),
        notes: ctx,
    });
    Ok(output(exec(ctx, command)?))
}
