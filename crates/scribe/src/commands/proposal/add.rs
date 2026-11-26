use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::proposal::{self, Command, exec};
use scribe_core::composition::Context;

pub(super) const ADD_EXAMPLES: &str = "Examples:\n  scribe proposal add --kind stale --older <id> --reason 'Review needed' --author opus";

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum Kind {
    Supersedes,
    Stale,
}

/// Arguments to `proposal add`.
#[derive(Debug, Args)]
pub(crate) struct AddArgs {
    /// Proposed change
    #[arg(long, value_enum)]
    kind: Kind,
    /// Entry to retire or supersede
    #[arg(long)]
    older: String,
    /// Replacement entry (required for supersedes)
    #[arg(long)]
    newer: Option<String>,
    /// Why this change is proposed
    #[arg(long)]
    reason: String,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
}

pub(super) fn run(args: AddArgs, ctx: &Context) -> Result<Output, CliError> {
    let kind = match args.kind {
        Kind::Stale => scribe_core::ProposalKind::Stale,
        Kind::Supersedes => scribe_core::ProposalKind::Supersedes,
    };
    let input = proposal::ProposalAddInput {
        kind,
        older: &args.older,
        newer: args.newer.as_deref(),
        reason: &args.reason,
        author: args.author,
    };
    Ok(output(exec(ctx, Command::Add(input))?, None, ctx))
}
