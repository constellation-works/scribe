//! Proposal listing and audited direct agent writes and settlement.

use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{Output, ProposalPayload, Record, Records};
use clap::{Args, Subcommand};

pub(super) const PROPOSAL_EXAMPLES: &str = "\
Examples:
  scribe proposal list
  scribe proposal list --json | jq '.proposals[] | select(.state == \"open\")'";

const LIST_EXAMPLES: &str = "\
Examples:
  scribe proposal list
  scribe proposal list --json";

/// Verbs on the `proposal` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum ProposalCommand {
    /// Open a proposal, including against a Daniel entry
    #[command(
        after_help = "Examples:\n  scribe proposal add --kind stale --older <id> --reason 'Review needed' --author opus"
    )]
    Add(AddArgs),
    /// Update the reason of an open agent proposal
    #[command(
        after_help = "Examples:\n  scribe proposal update <id> --author opus --reason 'New evidence'"
    )]
    Update(UpdateArgs),
    /// Approve or reject a proposal about an agent entry
    #[command(after_help = "Examples:\n  scribe proposal settle <id> --approve --author opus")]
    Settle(SettleArgs),
    /// List every proposal by id and its settlement state
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
}

/// Arguments to `proposal list` (none yet).
#[derive(Debug, Args)]
pub(crate) struct ListArgs {}

impl ProposalCommand {
    /// Audit every write; listings remain read-only.
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) => None,
            Self::Add(_) => Some(AuditMeta {
                command: "proposal add",
            }),
            Self::Update(_) => Some(AuditMeta {
                command: "proposal update",
            }),
            Self::Settle(_) => Some(AuditMeta {
                command: "proposal settle",
            }),
        }
    }
}

/// Run one `proposal` verb.
pub(super) fn run(command: ProposalCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        ProposalCommand::List(ListArgs {}) => list(ctx),
        ProposalCommand::Add(args) => {
            let redactor = ctx.redactor(&ctx.store().config()?)?;
            let kind = match args.kind {
                Kind::Stale => scribe_core::ProposalKind::Stale,
                Kind::Supersedes => scribe_core::ProposalKind::Supersedes,
            };
            let (proposal, changed) = ctx.store().add_agent_proposal(
                kind,
                &args.older,
                args.newer.as_deref(),
                &args.reason,
                args.author,
                ctx.now(),
                &redactor,
            )?;
            Ok(written(proposal, changed))
        }
        ProposalCommand::Update(args) => {
            let redactor = ctx.redactor(&ctx.store().config()?)?;
            let (proposal, changed) = ctx.store().change_agent_proposal(
                &args.id,
                scribe_core::ProposalChange::Update,
                Some(&args.reason),
                args.author,
                ctx.now(),
                &redactor,
            )?;
            Ok(written(proposal, changed))
        }
        ProposalCommand::Settle(args) => {
            let redactor = ctx.redactor(&ctx.store().config()?)?;
            let (proposal, changed) = ctx.store().change_agent_proposal(
                &args.id,
                if args.approve {
                    scribe_core::ProposalChange::Approve
                } else {
                    scribe_core::ProposalChange::Reject
                },
                args.reason.as_deref(),
                args.author,
                ctx.now(),
                &redactor,
            )?;
            Ok(written(proposal, changed))
        }
    }
}

fn list(ctx: &Context) -> Result<Output, CliError> {
    let proposals = ctx.store().proposals()?;
    let empty = format!(
        "no proposals in {}; the consistency routine opens them",
        ctx.store().root().display()
    );
    let notice = ListNotice {
        noun: "proposals",
        criteria: Vec::new(),
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
    /// Proposal id or unique prefix
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
}

fn written(proposal: scribe_core::Proposal, changed: bool) -> Output {
    Output::written(
        Record::Proposal(Box::new(ProposalPayload::from(&proposal))),
        changed,
    )
}
