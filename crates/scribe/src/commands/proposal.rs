//! The `proposal` noun: list what the consistency routine proposes. Read-only:
//! proposals are opened by the routine and settled only by `#scribe approve`
//! or `#scribe reject` in a typed turn, never by a command.

use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{Output, ProposalPayload, Records};
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
    /// List every proposal by id; settle one with #scribe approve or #scribe reject
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
}

/// Arguments to `proposal list` (none yet).
#[derive(Debug, Args)]
pub(crate) struct ListArgs {}

impl ProposalCommand {
    /// The audit entry for this verb: `None`, it reads (STD-01 R31).
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) => None,
        }
    }
}

/// Run one `proposal` verb.
pub(super) fn run(command: ProposalCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        ProposalCommand::List(ListArgs {}) => list(ctx),
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
