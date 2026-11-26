use super::outcome::output;
use crate::commands::{AuthorKindArg, DateArgs};
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::proposal::{self, Command, exec};
use scribe_core::composition::Context;
use scribe_core::{AuthorKind, ProposalFilter, ProposalState};

pub(super) const LIST_EXAMPLES: &str = "\
Examples:
  scribe proposal list
  scribe proposal list --state open --author-kind scribe
  scribe proposal list --entry <id> --since 2026-11-01
  scribe proposal list --json";

/// Arguments to `proposal list`.
#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Only proposals in this state [default: any]
    #[arg(long, value_name = "STATE", value_enum)]
    pub(crate) state: Option<StateArg>,

    /// Only proposals opened on a day in the range
    #[command(flatten)]
    pub(crate) dates: DateArgs,

    /// Only proposals about this entry, as the older or the newer one: its id or a unique prefix (at least four characters) [default: any]
    #[arg(long, value_name = "ID")]
    pub(crate) entry: Option<String>,

    /// Only proposals opened through this channel: user, agent (proposal add) or scribe (consistency or session pass) [default: any]
    #[arg(long, value_name = "KIND", value_enum)]
    pub(in crate::commands) author_kind: Option<AuthorKindArg>,
}

/// A proposal's settlement state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum StateArg {
    // Waiting to be settled.
    Open,
    // Applied.
    Approved,
    // Closed without applying.
    Rejected,
}

pub(super) fn run(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let outcome = exec(ctx, Command::List(list_input(&args)))?;
    Ok(output(outcome, Some(&args.dates), ctx))
}

fn list_input(args: &ListArgs) -> proposal::ListInput<'_> {
    proposal::ListInput {
        filter: ProposalFilter {
            state: args.state.map(|s| match s {
                StateArg::Open => ProposalState::Open,
                StateArg::Approved => ProposalState::Approved,
                StateArg::Rejected => ProposalState::Rejected,
            }),
            dates: scribe_core::DateRange::default(),
            entry: None,
            author_kind: args.author_kind.map(AuthorKind::from),
        },
        entry: args.entry.as_deref(),
        since: args.dates.since,
        until: args.dates.until,
    }
}
