//! The `session` noun: list and show the records of sessions the user switched
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
pub(in crate::commands) mod catalog;
pub(in crate::commands) mod pass;
pub(in crate::commands) mod rebuild;

use super::{DateArgs, ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{
    Kin, Output, Record, Records, SessionHandoffsPayload, SessionPayload, TreeRow,
};
use clap::{Args, Subcommand};
use scribe_core::{
    Project, Related, SessionFilter, SessionId, SessionOrder, SessionRecord, SessionStatus, Topic,
};
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
  scribe session list --since 2026-11-01 --status final --topic store --topic release
  scribe session list --related task:<id>
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
    /// List session records, newest first
    ///
    /// Lists session records, newest first by when each was last updated, with whether a pass is due; inactive sessions are included only with --include-inactive.
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
    /// Show one session record
    ///
    /// Shows one session record: its metadata, links, children, header and the log's table of contents.
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
    /// Write session records for due or named sessions
    ///
    /// Writes the record of each session whose pass is due, or of the sessions named, with the session model.
    #[command(after_help = pass::PASS_EXAMPLES)]
    Pass(pass::PassArgs),
    /// Rebuild a session record from its full transcript
    ///
    /// Replaces a session record with a log rebuilt from its transcript, resuming interrupted windows.
    #[command(after_help = rebuild::EXAMPLES)]
    Rebuild(rebuild::RebuildArgs),
    /// Hide an outdated session with a reason
    ///
    /// Hides a session whose content no longer describes something current, with a reason.
    #[command(after_help = activity::DEACTIVATE_EXAMPLES)]
    Deactivate(activity::DeactivateArgs),
    /// Show an inactive session or pin it against hiding
    ///
    /// Shows an inactive session again, or pins or unpins it against automatic deactivation.
    #[command(after_help = activity::ACTIVATE_EXAMPLES)]
    Activate(activity::ActivateArgs),
    /// Switch a past session on and catalog its transcript
    ///
    /// Switches a past session on by name and catalogs every window of its transcript.
    #[command(after_help = catalog::EXAMPLES)]
    Catalog(catalog::CatalogArgs),
}

/// Arguments to `session list`.
#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Only sessions that ran in this project (a repository slug) [default: any]
    #[arg(long, value_name = "PROJECT")]
    pub(crate) project: Option<Project>,

    /// Only session records of this group's members (id, unique prefix, or project group slug)
    #[arg(long, value_name = "ID")]
    pub(crate) group: Option<String>,

    /// Only sessions carrying this topic; repeat it to require every one [default: any]
    #[arg(long, value_name = "TOPIC")]
    pub(crate) topic: Vec<Topic>,

    /// Only sessions mentioning this reference (task:<id>, pr:<owner>/<repo>#<n>, repo:<slug> or session:<id>, whose id may be a unique prefix); repeat it to accept any of them [default: any]
    #[arg(long, value_name = "REF")]
    pub(crate) related: Vec<Related>,

    /// Only sessions with this status: open (more passes may update it) or final [default: any]
    #[arg(long, value_name = "STATUS", value_enum)]
    pub(crate) status: Option<StatusArg>,

    /// Only sessions that ran on a day in the range, from started to updated
    #[command(flatten)]
    pub(crate) dates: DateArgs,

    /// Show at most this many of the matching sessions, the most recently updated, 1 or more; --json reports the total and whether the list was cut [default: all]
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<NonZeroUsize>,

    /// Order the sessions as a tree: each one under the session it was spawned from (else the one it continues), with a PARENT column; --json keeps the envelope, in tree order
    #[arg(long)]
    pub(crate) tree: bool,

    /// Include inactive sessions (hidden by default), with an ACTIVE column
    #[arg(long)]
    pub(crate) include_inactive: bool,
}

/// A session record's status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum StatusArg {
    // More passes may update it.
    Open,
    // The session-end pass ran.
    Final,
}

impl From<StatusArg> for SessionStatus {
    fn from(status: StatusArg) -> Self {
        match status {
            StatusArg::Open => Self::Open,
            StatusArg::Final => Self::Final,
        }
    }
}

/// Arguments to `session show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The session's id, as printed by `session list`, or a unique prefix of it (at least four characters), or a cite: session:<id>, or session:<id>#<n> for one segment
    #[arg(value_name = "ID", value_parser = parse_session_ref)]
    pub(crate) id: SessionRef,

    /// Print every segment's text after the log's table of contents
    #[arg(long)]
    pub(crate) log: bool,

    /// Print only segment N of the log, with its text (from 1)
    #[arg(long, value_name = "N")]
    pub(crate) segment: Option<NonZeroU32>,
}

/// A session named by id (or a prefix of one) or by its cite, optionally
/// with a segment. The id is resolved against the store by the handler
/// ([`scribe_core::Store::resolve_session`]).
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
    /// The session and agent a write verb names, resolved before it runs, so
    /// a refusal is attributable. `list`, `show` and `pass` name none here: a
    /// pass's lines come from its outcomes.
    pub(super) fn subject(&self, ctx: &Context) -> AuditSubject {
        match self {
            Self::List(_) | Self::Show(_) | Self::Pass(_) => AuditSubject::default(),
            // A prefix is resolved before the line is written; an id that
            // resolves to no record leaves the target null, never the typed id.
            Self::Rebuild(args) => AuditSubject {
                target: ctx
                    .store()
                    .resolve_session(&args.id)
                    .ok()
                    .map(|id| format!("session:{id}")),
                author: Some(args.author.as_str().to_owned()),
            },
            Self::Catalog(args) => AuditSubject {
                target: crate::commands::transcript::resolve_transcript(ctx, &args.id)
                    .ok()
                    .map(|id| format!("session:{id}")),
                author: Some(args.author.as_str().to_owned()),
            },
            Self::Deactivate(args) => activity::subject(ctx, &args.id, &args.author),
            Self::Activate(args) => activity::subject(ctx, &args.id, &args.author),
        }
    }

    /// The audit entry for this verb: `None` for `list` and `show`, which
    /// read (STD-01 R31); `pass` writes records and `control.json`;
    /// `deactivate` and `activate` write one record.
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) | Self::Show(_) => None,
            Self::Rebuild(_) => Some(AuditMeta {
                command: rebuild::COMMAND,
                stepwise: true,
            }),
            Self::Pass(_) => Some(AuditMeta {
                command: pass::COMMAND,
                stepwise: true,
            }),
            Self::Deactivate(_) => Some(AuditMeta {
                command: activity::DEACTIVATE_COMMAND,
                stepwise: false,
            }),
            Self::Activate(_) => Some(AuditMeta {
                command: activity::ACTIVATE_COMMAND,
                stepwise: false,
            }),
            Self::Catalog(_) => Some(AuditMeta {
                command: catalog::COMMAND,
                stepwise: true,
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
        SessionCommand::Catalog(args) => catalog::run(&args, ctx),
    }
}

fn list(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let filter = SessionFilter {
        project: args.project,
        topics: args.topic,
        related: super::resolve_relateds(ctx, &args.related)?,
        status: args.status.map(SessionStatus::from),
        dates: args.dates.range()?,
        include_inactive: args.include_inactive,
        limit: args.limit,
        order: SessionOrder::Updated,
    };
    let store = ctx.store();
    let group = args
        .group
        .as_deref()
        .map(|id| store.group(id))
        .transpose()?;
    let members = group.as_ref().map(|group| group.members());
    let found = store.list_sessions_scoped(&filter, members.as_ref())?;
    let empty = format!(
        "no session records in {}; a session is cataloged after /scribe on",
        ctx.store().root().display()
    );
    let mut criteria = Vec::new();
    if let Some(project) = &filter.project {
        criteria.push(format!("--project {project}"));
    }
    if let Some(group) = group {
        criteria.push(format!("--group {}", group.id));
    }
    for topic in &filter.topics {
        criteria.push(format!("--topic {topic}"));
    }
    for related in &filter.related {
        criteria.push(format!("--related {related}"));
    }
    if let Some(status) = filter.status {
        criteria.push(format!("--status {}", status.as_str()));
    }
    criteria.extend(args.dates.criteria());
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
        .with_number(found.numbers.get(&r.id).copied())
        .with_forgotten(found.forgotten.get(&r.id).copied())
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
    let id = ctx.store().resolve_session_or_handoff(&args.id.id)?;
    let handoffs = ctx.store().handoffs_for_session(&id)?;
    let view = match ctx.store().session(&id) {
        Ok(view) => view,
        Err(scribe_core::Error::SessionNotFound { .. }) if !handoffs.is_empty() => {
            if let Some(n) = segment {
                return Err(CliError::SegmentNotFound {
                    session: id,
                    n: n.get(),
                    count: 0,
                });
            }
            let control = ctx.store().control()?;
            return Ok(Output::Show {
                record: Record::SessionHandoffs(Box::new(SessionHandoffsPayload::new(
                    &id,
                    control.get(&id),
                    &handoffs,
                ))),
            });
        }
        Err(err) => return Err(err.into()),
    };
    let record = &view.record;
    let payload = SessionPayload::new(
        record,
        view.control.as_ref(),
        Kin {
            children: &view.children,
            titles: &view.linked_titles,
        },
    )
    .with_number(view.number)
    .with_forgotten(view.forgotten)
    .with_handoffs(&handoffs);
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
