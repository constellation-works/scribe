//! Proposal listing and audited direct agent writes and settlement.

use super::{DateArgs, ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{Output, ProposalPayload, Record, Records};
use clap::{Args, Subcommand};
use scribe_core::cases::proposal_cmd;
use scribe_core::{AuthorKind, ProposalFilter, ProposalState};
use scribe_core::{EntryVerbs, ProposalVerbs};

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
    #[value(alias = "daniel")]
    User,
    // An agent, through proposal add.
    Agent,
    // scribe's consistency routine.
    Scribe,
}

impl ProposalCommand {
    /// The proposal and agent a write verb names, resolved before it runs, so
    /// a refusal is attributable. `add` names its target only in its output.
    pub(super) fn subject(&self, ctx: &Context) -> AuditSubject {
        match self {
            Self::List(_) | Self::Show(_) => AuditSubject::default(),
            Self::Add(args) => AuditSubject {
                target: None,
                author: Some(args.author.as_str().to_owned()),
            },
            Self::Update(args) => AuditSubject {
                target: resolved_proposal(ctx, &args.id),
                author: Some(args.author.as_str().to_owned()),
            },
            Self::Settle(args) => AuditSubject {
                target: resolved_proposal(ctx, &args.id),
                author: Some(args.author.as_str().to_owned()),
            },
        }
    }

    /// Audit every write; listings remain read-only.
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) | Self::Show(_) => None,
            Self::Add(_) => Some(AuditMeta {
                command: "proposal add",
                stepwise: false,
            }),
            Self::Update(_) => Some(AuditMeta {
                command: "proposal update",
                stepwise: false,
            }),
            Self::Settle(_) => Some(AuditMeta {
                command: "proposal settle",
                stepwise: false,
            }),
        }
    }
}

/// The id of the proposal `input` resolves to, or `None` when it does not.
fn resolved_proposal(ctx: &Context, input: &str) -> Option<String> {
    ctx.store()
        .resolve_proposal(input)
        .ok()
        .map(|proposal| proposal.id.to_string())
}

/// Run one `proposal` verb.
pub(super) fn run(command: ProposalCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        ProposalCommand::List(args) => list(&args, ctx),
        ProposalCommand::Show(args) => {
            let proposal = ctx.store().resolve_proposal(&args.id)?;
            Ok(Output::Show {
                record: Record::Proposal(Box::new(ProposalPayload::from(&proposal))),
            })
        }
        ProposalCommand::Add(args) => {
            let kind = match args.kind {
                Kind::Stale => scribe_core::ProposalKind::Stale,
                Kind::Supersedes => scribe_core::ProposalKind::Supersedes,
            };
            let (proposal, changed) = proposal_cmd::add(
                &ctx.core(),
                proposal_cmd::ProposalAddInput {
                    kind,
                    older: &args.older,
                    newer: args.newer.as_deref(),
                    reason: &args.reason,
                    author: args.author,
                },
            )?;
            Ok(written(proposal, changed))
        }
        ProposalCommand::Update(args) => {
            let (proposal, changed) =
                proposal_cmd::update(&ctx.core(), &args.id, &args.reason, args.author)?;
            Ok(written(proposal, changed))
        }
        ProposalCommand::Settle(args) => {
            args.instruction.note(ctx);
            scribe_core::check_agent_author(&args.author)?;
            if let Some(reason) = &args.reason {
                scribe_core::check_reason(reason)?;
            }
            let target = ctx.store().resolve_proposal(&args.id)?;
            // Before the store lock; a refusal changes nothing. A passing
            // check lets settle accept a candidate (2_design.md §12.1); a
            // proposal about Daniel's entry stays his.
            let instructed = super::instruction::check(
                ctx,
                &args.instruction,
                if args.approve {
                    scribe_core::instruction::Act::Approve
                } else {
                    scribe_core::instruction::Act::Reject
                },
                &scribe_core::instruction::Target::new(target.id.to_string(), &args.id),
            )?;
            let redactor = ctx.redactor(&ctx.store().read().tags().config()?)?;
            let (proposal, changed) = ctx.store().change_agent_proposal(
                &args.id,
                if args.approve {
                    scribe_core::ProposalChange::Approve
                } else {
                    scribe_core::ProposalChange::Reject
                },
                args.reason.as_deref(),
                instructed.as_ref(),
                args.author,
                ctx.now(),
                &redactor,
            )?;
            Ok(written(proposal, changed))
        }
    }
}

fn list(args: &ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let store = ctx.store();
    let entry = match &args.entry {
        Some(id) => Some(store.resolve_entry(id)?.id),
        None => None,
    };
    let filter = ProposalFilter {
        state: args.state.map(|s| match s {
            StateArg::Open => ProposalState::Open,
            StateArg::Approved => ProposalState::Approved,
            StateArg::Rejected => ProposalState::Rejected,
        }),
        dates: args.dates.range()?,
        entry,
        author_kind: args.author_kind.map(|k| match k {
            AuthorKindArg::User => AuthorKind::User,
            AuthorKindArg::Agent => AuthorKind::Agent,
            AuthorKindArg::Scribe => AuthorKind::Scribe,
        }),
    };
    let proposals: Vec<_> = store
        .read()
        .proposals()
        .all()?
        .into_iter()
        .filter(|p| filter.matches(p))
        .collect();
    let empty = format!(
        "no proposals in {}; the consistency routine opens them",
        store.root().display()
    );
    let mut criteria = Vec::new();
    if let Some(state) = filter.state {
        criteria.push(format!("--state {}", state.as_str()));
    }
    criteria.extend(args.dates.criteria());
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
    Ok(list_output(records, total, &[], &notice))
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

fn written(proposal: scribe_core::Proposal, changed: bool) -> Output {
    Output::written(
        Record::Proposal(Box::new(ProposalPayload::from(&proposal))),
        changed,
    )
}
