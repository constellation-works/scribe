//! The popup's callbacks: `tmux rows <view>` and `tmux preview <kind> <id>`.
//! Hidden from help, read-only and unaudited like the popup itself. Both
//! fetch records through the same commands a person would run
//! (`session list`, `entry show` …) by parsing their command line and
//! dispatching it, so what the popup shows is what those commands report and
//! no list or show output changes.

use crate::app::Context;
use crate::cli::Cli;
use crate::error::CliError;
use crate::output::{Output, PopupRows, Record, Records};
use clap::{Args, Subcommand, ValueEnum};
use std::ffi::OsString;

/// How many days back the backfill view looks.
const BACKFILL_DAYS: i64 = 30;

#[derive(Debug, Subcommand)]
pub(super) enum Internal {
    /// One list view as rows for fzf
    #[command(hide = true)]
    Rows(RowsArgs),
    /// One record's preview text
    #[command(hide = true)]
    Preview(PreviewArgs),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum View {
    Sessions,
    Entries,
    Proposals,
    Transcripts,
}

#[derive(Debug, Args)]
pub(super) struct RowsArgs {
    /// The list to show.
    #[arg(value_enum)]
    view: View,
    /// Include hidden records (inactive sessions; every entry status).
    #[arg(long)]
    hidden: bool,
    /// The popup's terminal can show color.
    #[arg(long)]
    color: bool,
    /// Treat this as the current year, so a date of another year shows its
    /// year (for deterministic dates in tests).
    #[arg(long, value_name = "YEAR")]
    year: Option<i32>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum Kind {
    Session,
    Entry,
    Proposal,
}

#[derive(Debug, Args)]
pub(super) struct PreviewArgs {
    /// The record kind.
    #[arg(value_enum)]
    kind: Kind,
    /// The record id.
    id: String,
}

pub(super) fn run(internal: Internal, ctx: &Context) -> Result<Output, CliError> {
    match internal {
        Internal::Rows(args) => rows(&args, ctx),
        Internal::Preview(args) => preview(&args, ctx),
    }
}

/// Run `scribe <argv…>` in this process and return its output.
fn dispatch(ctx: &Context, argv: &[&str]) -> Result<Output, CliError> {
    let mut full: Vec<OsString> = vec!["scribe".into()];
    full.extend(argv.iter().map(OsString::from));
    let cli = Cli::parse_args(&full).map_err(|e| CliError::Usage(e.to_string()))?;
    super::super::run(cli.command, ctx)
}

fn list(ctx: &Context, argv: &[&str]) -> Result<Records, CliError> {
    let Output::List { records, .. } = dispatch(ctx, argv)? else {
        return Err(CliError::Usage(format!(
            "scribe {} did not return a list",
            argv.join(" ")
        )));
    };
    Ok(records)
}

fn rows(args: &RowsArgs, ctx: &Context) -> Result<Output, CliError> {
    let year = args.year.unwrap_or_else(|| ctx.now().year());
    let (kind, listed) = match args.view {
        View::Sessions => (
            "session",
            list(
                ctx,
                if args.hidden {
                    &["session", "list", "--include-inactive"]
                } else {
                    &["session", "list"]
                },
            ),
        ),
        View::Entries => (
            "entry",
            list(
                ctx,
                if args.hidden {
                    &["entry", "list"]
                } else {
                    &["entry", "list", "--status", "active"]
                },
            ),
        ),
        View::Proposals => (
            "proposal",
            list(ctx, &["proposal", "list", "--state", "open"]),
        ),
        View::Transcripts => {
            let since = (ctx.now().date() - time::Duration::days(BACKFILL_DAYS)).to_string();
            (
                "transcript",
                list(ctx, &["transcript", "list", "--since", &since]),
            )
        }
    };
    let (records, notice) = match listed {
        Ok(records) if kind == "transcript" && records.len() == 0 => (
            None,
            Some("no uncataloged transcripts in the last 30 days".to_owned()),
        ),
        Ok(records) => (Some(records), None),
        Err(_) if kind == "transcript" => (
            None,
            Some(
                "backfill candidates are unavailable: scribe transcript list failed; run it in a shell for the reason"
                    .to_owned(),
            ),
        ),
        Err(_) => (
            None,
            Some(format!(
                "scribe {kind} list is unavailable; run it in a shell for the reason"
            )),
        ),
    };
    Ok(Output::PopupRows(PopupRows {
        kind,
        records,
        notice,
        year,
        color: args.color,
    }))
}

fn preview(args: &PreviewArgs, ctx: &Context) -> Result<Output, CliError> {
    let record = match args.kind {
        Kind::Session => show(ctx, &["session", "show", &args.id])?,
        Kind::Entry => show(ctx, &["entry", "show", &args.id])?,
        // There is no `proposal show`; the list carries every field.
        Kind::Proposal => {
            let Records::Proposals(all) = list(ctx, &["proposal", "list"])? else {
                return Err(CliError::Usage(
                    "proposal list returned no proposals".into(),
                ));
            };
            let found = all.into_iter().find(|p| p.id == args.id);
            let p =
                found.ok_or_else(|| CliError::Usage(format!("proposal {} not found", args.id)))?;
            Record::Proposal(Box::new(p))
        }
    };
    Ok(Output::PopupPreview(record))
}

fn show(ctx: &Context, argv: &[&str]) -> Result<Record, CliError> {
    let Output::Show { record } = dispatch(ctx, argv)? else {
        return Err(CliError::Usage(format!(
            "scribe {} did not return a record",
            argv.join(" ")
        )));
    };
    Ok(record)
}
