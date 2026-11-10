//! The `session` noun: list and show the records of sessions Daniel switched
//! on with `/scribe on`, and `pass`, which writes them. `show` and `list`
//! carry each record's `links` (stored on the child) and its `children`
//! (derived at read time from the other records' links); `list --tree`
//! orders the records by that nesting. `list` leaves inactive sessions out
//! unless `--include-inactive`; `show` prints the header and the log's
//! table of contents, `--log` every segment's text, and `--segment <n>` (or
//! the id written as the cite `session:<id>#<n>`) one segment. `list` and `show`
//! read only and make one store call each (STD-02 R2); `pass`, run by the
//! enabled routine or by hand, lives in `pass.rs`; `rebuild` replaces a
//! record from its transcript with resumable private checkpoints in `rebuild.rs`;
//! `deactivate` and `activate`, the `manual` route of the `active` flag, in
//! `activity.rs`.

pub(in crate::commands) mod activity;
pub(in crate::commands) mod pass;
pub(in crate::commands) mod rebuild;

use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{Kin, Output, Record, Records, SessionPayload, TreeRow};
use clap::{Args, Subcommand};
use scribe_core::{Project, SessionFilter, SessionId, SessionRecord, Topic};
use std::num::{NonZeroU32, NonZeroUsize};

pub(super) const SESSION_EXAMPLES: &str = "\
Examples:
  scribe session list --project scribe
  scribe session show <id>
  scribe session deactivate <id> --author opus --reason 'the feature was removed'
  scribe session pass --json";

const LIST_EXAMPLES: &str = "\
Examples:
  scribe session list
  scribe session list --tree
  scribe session list --topic release --limit 5
  scribe session list --include-inactive
  scribe session list --json | jq '.sessions[] | select(.due)'";

const SHOW_EXAMPLES: &str = "\
Examples:
  scribe session show <id>
  scribe session show <id> --log
  scribe session show <id> --segment 2
  scribe session show 'session:<id>#2'
  scribe session show <id> --json | jq -r '.log[].heading'";

/// Verbs on the `session` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum SessionCommand {
    /// List session records, oldest first, with whether a pass is due; inactive sessions only with --include-inactive
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
    /// Show one session record: its metadata, links, children, header and the log's table of contents
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
    /// Write the record of each session whose pass is due, or of the sessions named, with the session model
    #[command(after_help = pass::PASS_EXAMPLES)]
    Pass(pass::PassArgs),
    /// Replace a session record with a log rebuilt from its transcript, resuming interrupted windows
    #[command(after_help = rebuild::EXAMPLES)]
    Rebuild(rebuild::RebuildArgs),
    /// Hide a session whose content no longer describes something current, with a reason
    #[command(after_help = activity::DEACTIVATE_EXAMPLES)]
    Deactivate(activity::DeactivateArgs),
    /// Show an inactive session again, or pin or unpin it against automatic deactivation
    #[command(after_help = activity::ACTIVATE_EXAMPLES)]
    Activate(activity::ActivateArgs),
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

    /// Include inactive sessions (hidden by default), with an ACTIVE column
    #[arg(long)]
    pub(crate) include_inactive: bool,
}

/// Arguments to `session show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The session's id, as printed by `session list`, or a cite: session:<id>, or session:<id>#<n> for one segment
    #[arg(value_name = "ID", value_parser = parse_session_ref)]
    pub(crate) id: SessionRef,

    /// Print every segment's text after the log's table of contents
    #[arg(long)]
    pub(crate) log: bool,

    /// Print only segment N of the log, with its text (from 1)
    #[arg(long, value_name = "N")]
    pub(crate) segment: Option<NonZeroU32>,
}

/// A session named by id or by its cite, optionally with a segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionRef {
    pub(crate) id: SessionId,
    pub(crate) segment: Option<NonZeroU32>,
}

/// `<id>`, `session:<id>` or `session:<id>#<n>`.
fn parse_session_ref(raw: &str) -> Result<SessionRef, String> {
    let rest = raw.strip_prefix("session:").unwrap_or(raw);
    let (id, segment) = match rest.split_once('#') {
        Some((id, n)) => {
            let n = n.parse::<NonZeroU32>().map_err(|_| {
                format!("invalid segment '{n}' in '{raw}': expected session:<id>#<n> with n from 1")
            })?;
            (id, Some(n))
        }
        None => (rest, None),
    };
    let id = id.parse::<SessionId>().map_err(|e| e.to_string())?;
    Ok(SessionRef { id, segment })
}

impl SessionCommand {
    /// The audit entry for this verb: `None` for `list` and `show`, which
    /// read (STD-01 R31); `pass` writes records and `control.json`;
    /// `deactivate` and `activate` write one record.
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) | Self::Show(_) => None,
            Self::Rebuild(_) => Some(AuditMeta {
                command: rebuild::COMMAND,
            }),
            Self::Pass(_) => Some(AuditMeta {
                command: pass::COMMAND,
            }),
            Self::Deactivate(_) => Some(AuditMeta {
                command: activity::DEACTIVATE_COMMAND,
            }),
            Self::Activate(_) => Some(AuditMeta {
                command: activity::ACTIVATE_COMMAND,
            }),
        }
    }
}

/// Run one `session` verb.
pub(super) fn run(command: SessionCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        SessionCommand::List(args) => list(args, ctx),
        SessionCommand::Show(args) => show(&args, ctx),
        SessionCommand::Rebuild(args) => rebuild::run(&args, ctx),
        SessionCommand::Pass(args) => pass::run(&args, ctx),
        SessionCommand::Deactivate(args) => activity::deactivate(args, ctx),
        SessionCommand::Activate(args) => activity::activate(args, ctx),
    }
}

fn list(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let filter = SessionFilter {
        project: args.project,
        topic: args.topic,
        include_inactive: args.include_inactive,
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
    let active_column = args.include_inactive;
    let records = if args.tree {
        Records::SessionTree {
            rows: scribe_core::tree_order(items)
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
            active_column,
        }
    } else {
        Records::Sessions {
            items: items.iter().map(payload).collect(),
            active_column,
        }
    };
    let mut output = list_output(records, found.listing.total, &found.skipped, &notice);
    if found.inactive_hidden > 0
        && let Output::List { notices, .. } = &mut output
    {
        notices.push(format!(
            "{} inactive session(s) also match and are hidden; --include-inactive lists them",
            found.inactive_hidden
        ));
    }
    Ok(output)
}

/// One record, inactive or not. The JSON carries every segment with its
/// text, or the one asked for; the table prints the table of contents, and
/// the text with `--log` or a segment.
fn show(args: &ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    let segment = match (args.segment, args.id.segment) {
        (Some(flag), Some(cite)) if flag != cite => {
            return Err(CliError::Usage(format!(
                "--segment {flag} and the cite's #{cite} name different segments; give one"
            )));
        }
        (flag, cite) => flag.or(cite),
    };
    let view = ctx.store().session(&args.id.id)?;
    let record = &view.record;
    let payload = SessionPayload::new(
        record,
        view.control.as_ref(),
        Kin {
            children: &view.children,
            titles: &view.linked_titles,
        },
    );
    let mut payload = match segment {
        Some(n) => {
            let one = record
                .segment(n.get())
                .ok_or_else(|| CliError::SegmentNotFound {
                    session: record.id.clone(),
                    n: n.get(),
                    count: record.log.len(),
                })?;
            payload.with_log([one])
        }
        None => payload.with_log(&record.log),
    };
    payload.log_text = args.log || segment.is_some();
    Ok(Output::Show {
        record: Record::Session(Box::new(payload)),
    })
}

#[cfg(test)]
mod tests;
