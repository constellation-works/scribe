//! The `session` noun: list and show the records of sessions Daniel switched
//! on with `/scribe on`. Read-only: records are written only by the
//! session-pass routine. Each verb makes one store call (STD-02 R2).

use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{Output, Record, Records, SessionPayload};
use clap::{Args, Subcommand};
use scribe_core::{Project, SessionFilter, SessionId, Topic};
use std::num::NonZeroUsize;

pub(super) const SESSION_EXAMPLES: &str = "\
Examples:
  scribe session list --project scribe
  scribe session show <id>";

const LIST_EXAMPLES: &str = "\
Examples:
  scribe session list
  scribe session list --topic release --limit 5
  scribe session list --json | jq '.sessions[] | select(.due)'";

const SHOW_EXAMPLES: &str = "\
Examples:
  scribe session show <id>
  scribe session show <id> --json | jq -r .next";

/// Verbs on the `session` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum SessionCommand {
    /// List session records, oldest first, with whether a pass is due
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
    /// Show one session record in full: its metadata and its four sections
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
}

/// Arguments to `session list`.
#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Only sessions that ran in this project (a repository slug) [default: any]
    #[arg(long, value_name = "PROJECT")]
    pub(crate) project: Option<Project>,

    /// Only sessions carrying this topic [default: any]
    #[arg(long, value_name = "TOPIC")]
    pub(crate) topic: Option<Topic>,

    /// Show at most this many of the matching sessions, 1 or more; --json reports the total and whether the list was cut [default: all]
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<NonZeroUsize>,
}

/// Arguments to `session show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The session's id, as printed by `session list`
    #[arg(value_name = "ID")]
    pub(crate) id: SessionId,
}

impl SessionCommand {
    /// The audit entry for this verb: `None`, both verbs read (STD-01 R31).
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) | Self::Show(_) => None,
        }
    }
}

/// Run one `session` verb.
pub(super) fn run(command: SessionCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        SessionCommand::List(args) => list(args, ctx),
        SessionCommand::Show(args) => show(&args, ctx),
    }
}

fn list(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let filter = SessionFilter {
        project: args.project,
        topic: args.topic,
        limit: args.limit,
    };
    let found = ctx.store().list_sessions(&filter)?;
    let empty = format!(
        "no session records in {}; a session is cataloged after /scribe on",
        ctx.store().root().display()
    );
    let mut criteria = Vec::new();
    if let Some(project) = &filter.project {
        criteria.push(format!("--project {project}"));
    }
    if let Some(topic) = &filter.topic {
        criteria.push(format!("--topic {topic}"));
    }
    let notice = ListNotice {
        noun: "sessions",
        criteria,
        empty: &empty,
    };
    let records = Records::Sessions(
        found
            .listing
            .items
            .iter()
            .map(|r| SessionPayload::new(r, found.control.get(&r.id)))
            .collect(),
    );
    Ok(list_output(
        records,
        found.listing.total,
        &found.skipped,
        &notice,
    ))
}

fn show(args: &ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    let view = ctx.store().session(&args.id)?;
    Ok(Output::Show {
        record: Record::Session(Box::new(SessionPayload::new(
            &view.record,
            view.control.as_ref(),
        ))),
    })
}
