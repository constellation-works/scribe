//! `scribe mark`: the `#scribe` markers scribe wrote no entry for
//! (docs/design/scribe/2_design.md §14.5). `list` shows each one handed to
//! the agent to record (`agent_handoff`), and each left from before
//! ORB-27911: pending (its session's next `Stop` hands it over) or given up
//! after every model call failed (`model_failed`). It reads only.

use super::{ListNotice, list_output};
use crate::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{Output, Records};
use clap::{Args, Subcommand};
use scribe_core::SessionId;
use scribe_core::apps::mark::{Command, Outcome, exec};

pub(super) const EXAMPLES: &str = "\
Examples:
  scribe mark list
  scribe mark list --session <id> --json";

const LIST_EXAMPLES: &str = "\
A marker is listed when scribe wrote no entry for it: handed to the agent to
record (outcome refused, code agent_handoff), or left from before scribe
stopped calling a model for markers: pending (the session's next reply hands
it over) or given up (code model_failed). It holds ids only; the line it
marks is in the session's transcript. Whether the agent recorded it is in
scribe entry list.

Examples:
  scribe mark list
  scribe mark list --session <id> --json";

/// Verbs on the `mark` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum MarkCommand {
    /// List #scribe markers with no entry from scribe
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
}

/// Arguments to `mark list`.
#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Only this session's markers: a provider session id or a unique prefix [default: every session]
    #[arg(long, value_name = "SESSION")]
    session: Option<SessionId>,
}

impl MarkCommand {
    /// `list` reads only.
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) => None,
        }
    }

    /// `list` names no record.
    pub(super) fn subject(&self, _ctx: &Context) -> AuditSubject {
        match self {
            Self::List(_) => AuditSubject::default(),
        }
    }
}

pub(super) fn run(command: MarkCommand, ctx: &Context) -> Result<Output, CliError> {
    let command = match command {
        MarkCommand::List(args) => Command::List {
            session: args.session,
            provider: ctx.provider(),
        },
    };
    Ok(match exec(&ctx.runtime(), command)? {
        Outcome::List { session, marks } => list(session, marks),
    })
}

/// Read-only: no lock, no audit, no write.
fn list(session: Option<SessionId>, marks: Vec<scribe_core::annotation::MarkRecord>) -> Output {
    let notice = ListNotice {
        noun: "#scribe markers with no entry from scribe",
        criteria: session.iter().map(|id| format!("--session {id}")).collect(),
        empty: "no #scribe markers without an entry from scribe",
    };
    let total = marks.len();
    list_output(Records::Marks(marks), total, &[], &notice)
}
