//! The `session` noun: list and show the records of sessions Daniel switched
//! on with `/scribe on`, and `pass`, which writes them. `show` and `list`
//! carry each record's `links` (stored on the child) and its `children`
//! (derived at read time from the other records' links); `list --tree`
//! orders the records by that nesting. `list` and `show`
//! read only and make one store call each (STD-02 R2); `pass`, run by the
//! enabled routine or by hand, lives in `pass.rs`.

pub(in crate::commands) mod pass;

use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{Kin, Output, Record, Records, SessionPayload, TreeRow};
use clap::{Args, Subcommand};
use scribe_core::{Project, SessionFilter, SessionId, SessionRecord, Topic};
use std::num::NonZeroUsize;

pub(super) const SESSION_EXAMPLES: &str = "\
Examples:
  scribe session list --project scribe
  scribe session show <id>
  scribe session pass --json";

const LIST_EXAMPLES: &str = "\
Examples:
  scribe session list
  scribe session list --tree
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
    /// Show one session record in full: its metadata, links, children and four sections
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
    /// Write the record of each session whose pass is due, or of the sessions named, with the session model
    #[command(after_help = pass::PASS_EXAMPLES)]
    Pass(pass::PassArgs),
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

    /// Order the sessions as a tree: each one under the session it was spawned from (else the one it continues), with a PARENT column; --json keeps the envelope, in tree order
    #[arg(long)]
    pub(crate) tree: bool,
}

/// Arguments to `session show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The session's id, as printed by `session list`
    #[arg(value_name = "ID")]
    pub(crate) id: SessionId,
}

impl SessionCommand {
    /// The audit entry for this verb: `None` for `list` and `show`, which
    /// read (STD-01 R31); `pass` writes records and `control.json`.
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) | Self::Show(_) => None,
            Self::Pass(_) => Some(AuditMeta {
                command: pass::COMMAND,
            }),
        }
    }
}

/// Run one `session` verb.
pub(super) fn run(command: SessionCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        SessionCommand::List(args) => list(args, ctx),
        SessionCommand::Show(args) => show(&args, ctx),
        SessionCommand::Pass(args) => pass::run(&args, ctx),
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
    let payload = |r: &SessionRecord| {
        SessionPayload::new(
            r,
            found.control.get(&r.id),
            Kin::of(&r.id, &found.children, &found.titles),
        )
    };
    let items = &found.listing.items;
    let records = if args.tree {
        Records::SessionTree(
            scribe_core::tree_order(items)
                .into_iter()
                .map(|(depth, i)| {
                    let record = &items[i];
                    TreeRow {
                        depth,
                        parent: scribe_core::tree_parent(record).map(ToString::to_string),
                        session: payload(record),
                    }
                })
                .collect(),
        )
    } else {
        Records::Sessions(items.iter().map(payload).collect())
    };
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
            Kin {
                children: &view.children,
                titles: &view.linked_titles,
            },
        ))),
    })
}

#[cfg(test)]
mod tests;
