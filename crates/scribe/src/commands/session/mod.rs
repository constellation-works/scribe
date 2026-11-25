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
use crate::error::CliError;
use crate::output::{
    Kin, Output, Record, Records, SessionHandoffsPayload, SessionPayload, TreeRow,
};
use clap::{Args, Subcommand};
use scribe_core::apps::session::{self as app, Command, ModelsOnce, Outcome, Shown, exec};
use scribe_core::composition::Context;
use scribe_core::{
    Config, Models, Project, Related, SessionFilter, SessionOrder, SessionRecord, SessionStatus,
    Topic,
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
    /// Settle due or named sessions when no new window needs distillation
    ///
    /// Settles eligible sessions deterministically and prints the Orbit job command when new windows need distillation.
    #[command(after_help = pass::PASS_EXAMPLES)]
    Pass(pass::PassArgs),
    /// Prepare one session window or settle a session without a model
    ///
    /// Reads a selection and optional continuation as JSON on stdin. Writes a prepared window, settlement, or completion as JSON.
    PassPrepare,
    /// Verify a supplied session answer and apply it or request a retry
    ///
    /// Reads the prepared basis, cursor and answer as JSON on stdin. Writes the apply status, cursor and any retry instructions as JSON.
    PassApply,
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

pub(crate) use scribe_core::apps::session::{SessionRef, parse_session_ref};

/// A session app entry point: its `exec`, which audits the commands that
/// write, or in tests its unaudited one.
type Exec = fn(&Context, Command<'_>) -> Result<Outcome, scribe_core::CommandError>;

/// Run one `session` verb.
pub(super) fn run(command: SessionCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        SessionCommand::List(args) => list(args, ctx),
        SessionCommand::Show(args) => show(&args, ctx),
        SessionCommand::Rebuild(args) => rebuild::run(&args, ctx),
        SessionCommand::Pass(args) => pass::run(args, ctx),
        SessionCommand::PassPrepare => pass::step_stdin("session-pass-prepare", ctx),
        SessionCommand::PassApply => pass::step_stdin("session-pass-apply", ctx),
        SessionCommand::Deactivate(args) => activity::deactivate(args, ctx),
        SessionCommand::Activate(args) => activity::activate(args, ctx),
        SessionCommand::Catalog(args) => catalog::run(&args, ctx),
    }
}

fn list(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let input = app::ListInput {
        filter: SessionFilter {
            project: args.project.clone(),
            topics: args.topic.clone(),
            related: args.related.clone(),
            status: args.status.map(SessionStatus::from),
            dates: scribe_core::DateRange::default(),
            include_inactive: args.include_inactive,
            limit: args.limit,
            order: SessionOrder::Updated,
        },
        since: args.dates.since,
        until: args.dates.until,
        group: args.group.clone(),
        provider: ctx.provider(),
    };
    let outcome = exec(ctx, Command::List(Box::new(input)))?;
    Ok(output(
        outcome,
        View {
            list: Some(&args),
            ..View::default()
        },
        ctx,
    ))
}

/// `session list`'s output, echoing `args` as criteria.
fn listed(listed: app::Listed, args: Option<&ListArgs>, ctx: &Context) -> Output {
    let (filter, group, found) = (listed.filter, listed.group, listed.found);
    let empty = format!(
        "no session records in {}; a session is cataloged after /scribe on",
        ctx.root().display()
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
    if let Some(args) = args {
        criteria.extend(args.dates.criteria());
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
        .with_number(found.numbers.get(&r.id).copied())
        .with_forgotten(found.forgotten.get(&r.id).copied())
    };
    let items = &found.listing.items;
    let active_column = filter.include_inactive;
    let records = if args.is_some_and(|args| args.tree) {
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
    output
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
    let outcome = exec(
        ctx,
        Command::Show {
            id: args.id.id.clone(),
            segment,
        },
    )?;
    Ok(output(
        outcome,
        View {
            log: args.log,
            segment,
            ..View::default()
        },
        ctx,
    ))
}

/// What a `session show` read: the record, its table of contents, and the
/// text with `log` or a `segment`.
fn shown(shown: Shown, log: bool, segment: Option<NonZeroU32>) -> Output {
    let (view, handoffs) = match shown {
        Shown::Record { view, handoffs } => (view, handoffs),
        Shown::Handoffs {
            id,
            control,
            handoffs,
        } => {
            return Output::Show {
                record: Record::SessionHandoffs(Box::new(SessionHandoffsPayload::new(
                    &id,
                    control.as_ref(),
                    &handoffs,
                ))),
            };
        }
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
        // The case refused a segment the log does not have.
        Some(n) => payload.with_log(record.segment(n.get())),
        None => payload.with_log(&record.log),
    };
    payload.log_text = log || segment.is_some();
    Output::Show {
        record: Record::Session(Box::new(payload)),
    }
}

/// What the flags ask of an outcome's output beyond the outcome itself.
#[derive(Default)]
pub(in crate::commands) struct View<'a> {
    /// `session list`'s arguments, echoed as criteria.
    pub(in crate::commands) list: Option<&'a ListArgs>,
    /// `session show --log`.
    pub(in crate::commands) log: bool,
    /// `session show`'s segment.
    pub(in crate::commands) segment: Option<NonZeroU32>,
    /// `transcript list`'s criteria.
    pub(in crate::commands) transcripts: &'a [String],
}

/// A `session` or `transcript` outcome as its output.
pub(in crate::commands) fn output(outcome: Outcome, view: View<'_>, ctx: &Context) -> Output {
    match outcome {
        Outcome::Listed(found) => listed(*found, view.list, ctx),
        Outcome::Shown(read) => shown(*read, view.log, view.segment),
        Outcome::Passed(report) => Output::Pass(*report),
        Outcome::Activity(written) => activity::output(*written),
        Outcome::Transcripts(found) => crate::commands::transcript::listed(found, view.transcripts),
        Outcome::Transcript(surveyed) => crate::commands::transcript::shown(*surveyed),
        Outcome::PassPrepared(_) | Outcome::PassApplied(_) | Outcome::PassFailed(_) => {
            unreachable!("pass steps use their JSON handler")
        }
    }
}

/// A `pass`, `catalog` or `rebuild` outcome as its output.
fn passed(outcome: Outcome, ctx: &Context) -> Output {
    output(outcome, View::default(), ctx)
}

/// `models` as the boxed one-shot factory a command carries.
pub(in crate::commands) fn once<'i, M: Models + 'i>(
    models: impl FnOnce(&Config) -> M + 'i,
) -> ModelsOnce<'i> {
    Box::new(move |config: &Config| Box::new(models(config)) as Box<dyn Models + 'i>)
}

#[cfg(test)]
mod tests;
