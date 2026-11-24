//! `scribe transcript list` and `transcript show`: model-free readings of
//! Claude Code transcripts that have no record and no switch
//! (docs/design/scribe/2_design.md §5.8).
//!
//! Both read transcripts and the store and write nothing: no lock, no audit
//! line, and they work on a read-only root. `list` is how the user picks a
//! past session to catalog; `show` is the preview.

use super::{DateArgs, ListNotice, day_parser};
use crate::app::Context;
use crate::error::CliError;
use crate::output::{Output, Record, Records, TranscriptPayload, WarningPayload};
use clap::{Args, Subcommand};
use scribe_core::cases::transcript::Surveyed;
use scribe_core::{DateRange, Project, SessionId};
use time::Date;

/// The default `--min-turns`: long sessions only.
const DEFAULT_MIN_TURNS: u32 = 20;

pub(super) const EXAMPLES: &str = "\
Lists provider transcripts that have no session record and no switch,
selected with --provider codex or claude-code (the default),
newest first, and writes nothing. --min-turns defaults to 20. project is
the configured [projects.<slug>] path with the most mentions, else the
cwd's project. show prints one transcript with the user's first five turns.

Examples:
  scribe transcript list --since 2026-10-01 --project scribe
  scribe transcript list --since 2026-10-01 --until 2026-10-31 --min-turns 40
  scribe transcript show <id>
  scribe transcript list --since 2026-10-01 --json | jq '.transcripts[].session'";

const LIST_EXAMPLES: &str = "\
Examples:
  scribe transcript list --since 2026-10-01
  scribe transcript list --since 2026-10-01 --project orbit --project scribe
  scribe transcript list --since 2026-10-01 --min-turns 40 --limit 10
  scribe transcript show <id>";

const SHOW_EXAMPLES: &str = "\
Examples:
  scribe transcript show <id>
  scribe transcript show <id> --json | jq '.user_first_turns'";

/// Verbs on the `transcript` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum TranscriptCommand {
    /// List uncataloged transcripts, newest first, writing nothing
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
    /// Show one transcript and the user's first five turns, writing nothing
    #[command(after_help = SHOW_EXAMPLES)]
    Show(ShowArgs),
}

/// Arguments to `transcript list`.
#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Only transcripts whose span, from the first message to the last, overlaps this day, YYYY-MM-DD
    #[arg(long, value_name = "DATE", value_parser = day_parser)]
    pub(crate) since: Date,

    /// Only transcripts on or before this day, YYYY-MM-DD [default: no upper bound]
    #[arg(long, value_name = "DATE", value_parser = day_parser)]
    pub(crate) until: Option<Date>,

    /// Only transcripts attributed to this project; repeat to accept any of them [default: any]
    #[arg(long, value_name = "PROJECT")]
    pub(crate) project: Vec<Project>,

    /// Only transcripts with at least this many of the user's turns
    #[arg(long, value_name = "N", default_value_t = DEFAULT_MIN_TURNS)]
    pub(crate) min_turns: u32,

    /// Show at most this many transcripts, 1 or more; --json reports the total and whether the list was cut [default: all]
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<std::num::NonZeroUsize>,
}

/// Arguments to `transcript show`.
#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    /// The transcript's id, or a unique prefix of it (at least four characters)
    #[arg(value_name = "ID")]
    pub(crate) id: SessionId,
}

impl TranscriptCommand {
    /// `list` and `show` read only, so they are not audited (STD-01 R31).
    pub(super) fn audit(&self) -> Option<crate::audit_middleware::AuditMeta> {
        match self {
            Self::List(_) | Self::Show(_) => None,
        }
    }
}

/// Run one `transcript` verb.
pub(super) fn run(command: TranscriptCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        TranscriptCommand::List(args) => list(args, ctx),
        TranscriptCommand::Show(args) => show(&args, ctx),
    }
}

/// The transcript `input` names, resolved against filenames under the
/// transcripts directory. Reads only.
pub(in crate::commands) fn resolve_transcript(
    ctx: &Context,
    input: &SessionId,
) -> Result<SessionId, CliError> {
    Ok(scribe_core::cases::catalog::resolve_transcript(
        &ctx.core(),
        ctx.provider(),
        input,
    )?)
}

fn list(args: ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let dates = dates(args.since, args.until)?;
    let mut criteria = Vec::new();
    criteria.extend(
        DateArgs {
            since: Some(args.since),
            until: args.until,
        }
        .criteria(),
    );
    for project in &args.project {
        criteria.push(format!("--project {project}"));
    }
    criteria.push(format!("--min-turns {}", args.min_turns));
    let Some(listed) = scribe_core::cases::transcript::list(
        &ctx.core(),
        ctx.provider(),
        &scribe_core::cases::transcript::ListInput {
            since: args.since,
            dates,
            projects: &args.project,
            min_turns: args.min_turns,
            limit: args.limit,
        },
    )?
    else {
        return Ok(Output::List {
            notices: vec![String::from(
                "no transcripts directory: set HOME, CLAUDE_CONFIG_DIR or CODEX_HOME for the selected provider",
            )],
            records: Records::Transcripts(Vec::new()),
            total: 0,
            warnings: Vec::new(),
            message_matches: None,
        });
    };
    let total = listed.total;
    let rows: Vec<TranscriptPayload> = listed
        .rows
        .into_iter()
        .map(|row| payload(row, false))
        .collect();
    let warnings = listed
        .warnings
        .into_iter()
        .map(|(path, error)| WarningPayload {
            path,
            code: "io_error",
            error,
        })
        .collect();
    let notice = ListNotice {
        noun: "transcripts",
        criteria,
        empty: "no uncataloged transcripts",
    };
    Ok(Output::List {
        notices: notice.line(rows.len(), total).into_iter().collect(),
        records: Records::Transcripts(rows),
        total,
        warnings,
        message_matches: None,
    })
}

fn show(args: &ShowArgs, ctx: &Context) -> Result<Output, CliError> {
    let surveyed = scribe_core::cases::transcript::show(&ctx.core(), ctx.provider(), &args.id)?;
    Ok(Output::Show {
        record: Record::Transcript(Box::new(payload(surveyed, true))),
    })
}

/// A survey as its row; `with_turns` adds the user's first turns.
fn payload(surveyed: Surveyed, with_turns: bool) -> TranscriptPayload {
    let survey = surveyed.survey;
    TranscriptPayload {
        session: surveyed.session.to_string(),
        project: surveyed.project.to_string(),
        project_mentions: surveyed
            .mentions
            .iter()
            .map(|(project, count)| (project.to_string(), *count))
            .collect(),
        cwd: surveyed.cwd,
        started: survey.started,
        ended: survey.ended,
        bytes: survey.bytes,
        user_turns: survey.user_turns,
        first_prompt: survey.first_prompt,
        user_first_turns: with_turns.then_some(survey.first_turns),
    }
}

fn dates(since: Date, until: Option<Date>) -> Result<DateRange, CliError> {
    DateArgs {
        since: Some(since),
        until,
    }
    .range()
}
