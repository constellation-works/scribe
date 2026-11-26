//! Proposal listing and audited direct agent writes and settlement.

use super::{DateArgs, ListNotice, list_output};
use crate::error::CliError;
use crate::output::{Output, ProposalPayload, Record, Records};
use clap::{Args, Subcommand};
use scribe_core::apps::proposal::{self, Command, Outcome, exec};
use scribe_core::composition::Context;
use scribe_core::{AuthorKind, ProposalFilter, ProposalState};

pub(super) const PROPOSAL_EXAMPLES: &str = "\
Examples:
  scribe proposal list
  scribe proposal show <id>
  scribe proposal list --json | jq '.proposals[] | select(.state == \"open\")'";

const SHOW_EXAMPLES: &str = "\
Examples:
  scribe proposal show p-7
  scribe proposal show p-7 --json | jq '.candidate.context'";

const LIST_EXAMPLES: &str = "\
Examples:
  scribe proposal list
  scribe proposal list --state open --author-kind scribe
  scribe proposal list --entry <id> --since 2026-11-01
  scribe proposal list --json";

/// Verbs on the `proposal` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum ProposalCommand {
    /// Open a proposal, including against an entry of the user's
    #[command(
        after_help = "Examples:\n  scribe proposal add --kind stale --older <id> --reason 'Review needed' --author opus"
    )]
    Add(AddArgs),
    /// Update the reason of an open agent proposal
    #[command(
        after_help = "Examples:\n  scribe proposal update <id> --author opus --reason 'New evidence'"
    )]
    Update(UpdateArgs),
    /// Approve or reject a proposal or candidate
    ///
    /// Approves or rejects a proposal about an agent entry, or a claim on the user's instruction.
    #[command(
        after_help = "Examples:\n  scribe proposal settle <id> --approve --author opus\n  scribe proposal settle <id> --reject --author opus --session <id> --instruction '<their words>'"
    )]
    Settle(SettleArgs),
    /// List every proposal by id and its settlement state
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
    /// Show one proposal in full: a claim's context, subject and quote
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
}

/// Arguments to `proposal show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The proposal: its id (`p-<n>`)
    pub(crate) id: String,
}

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
    pub(crate) author_kind: Option<AuthorKindArg>,
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

/// Who opened a proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum AuthorKindArg {
    // The user, through a typed #scribe line.
    User,
    // An agent, through proposal add.
    Agent,
    // scribe's consistency routine.
    Scribe,
}

/// Run one `proposal` verb.
pub(super) fn run(command: ProposalCommand, ctx: &Context) -> Result<Output, CliError> {
    let outcome = match command {
        ProposalCommand::List(args) => {
            let outcome = exec(ctx, Command::List(list_input(&args)))?;
            return Ok(output(outcome, Some(&args), ctx));
        }
        ProposalCommand::Show(args) => exec(ctx, Command::Show(args.id))?,
        ProposalCommand::Add(args) => {
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
            exec(ctx, Command::Add(input))?
        }
        ProposalCommand::Update(args) => exec(
            ctx,
            Command::Update {
                id: args.id,
                reason: args.reason,
                author: args.author,
            },
        )?,
        ProposalCommand::Settle(args) => {
            let input = proposal::ProposalSettleInput {
                id: &args.id,
                approve: args.approve,
                reason: args.reason.as_deref(),
                author: args.author,
                instruction: args.instruction.case(ctx),
            };
            exec(ctx, Command::Settle(input))?
        }
    };
    Ok(output(outcome, None, ctx))
}

/// The output of `outcome`; a list echoes `args` as criteria.
fn output(outcome: Outcome, args: Option<&ListArgs>, ctx: &Context) -> Output {
    match outcome {
        Outcome::Listed { filter, proposals } => list(&filter, &proposals, args, ctx),
        Outcome::Shown(proposal) => Output::Show {
            record: Record::Proposal(Box::new(ProposalPayload::from(&*proposal))),
        },
        Outcome::Written(proposal, changed) => written(&proposal, changed),
    }
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
            author_kind: args.author_kind.map(|k| match k {
                AuthorKindArg::User => AuthorKind::User,
                AuthorKindArg::Agent => AuthorKind::Agent,
                AuthorKindArg::Scribe => AuthorKind::Scribe,
            }),
        },
        entry: args.entry.as_deref(),
        since: args.dates.since,
        until: args.dates.until,
    }
}

fn list(
    filter: &ProposalFilter,
    proposals: &[scribe_core::Proposal],
    args: Option<&ListArgs>,
    ctx: &Context,
) -> Output {
    let empty = format!(
        "no proposals in {}; the consistency routine opens them",
        ctx.root().display()
    );
    let mut criteria = Vec::new();
    if let Some(state) = filter.state {
        criteria.push(format!("--state {}", state.as_str()));
    }
    if let Some(args) = args {
        criteria.extend(args.dates.criteria());
    }
    if let Some(entry) = &filter.entry {
        criteria.push(format!("--entry {entry}"));
    }
    if let Some(kind) = filter.author_kind {
        criteria.push(format!("--author-kind {}", kind.as_str()));
    }
    let notice = ListNotice {
        noun: "proposals",
        criteria,
        empty: &empty,
    };
    let total = proposals.len();
    let records = Records::Proposals(proposals.iter().map(ProposalPayload::from).collect());
    list_output(records, total, &[], &notice)
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum Kind {
    Supersedes,
    Stale,
}

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

#[derive(Debug, Args)]
pub(crate) struct UpdateArgs {
    /// Proposal id or unique prefix (at least four characters)
    id: String,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Replacement reason
    #[arg(long)]
    reason: String,
}

#[derive(Debug, Args)]
pub(crate) struct SettleArgs {
    /// Proposal id or unique prefix
    id: String,
    /// Agent name
    #[arg(long)]
    author: scribe_core::AuthorName,
    /// Apply the proposal
    #[arg(long, required_unless_present = "reject", conflicts_with = "reject")]
    approve: bool,
    /// Close without applying
    #[arg(long, required_unless_present = "approve")]
    reject: bool,
    /// Replacement explanation for the settlement
    #[arg(long)]
    reason: Option<String>,
    #[command(flatten)]
    instruction: super::instruction::InstructionArgs,
}

fn written(proposal: &scribe_core::Proposal, changed: bool) -> Output {
    Output::written(
        Record::Proposal(Box::new(ProposalPayload::from(proposal))),
        changed,
    )
}
