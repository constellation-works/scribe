//! The root of the command tree: the program, its global flags and the
//! parse entry point. Each noun's subcommands, flags and help live beside its
//! handlers in `commands/` (STD-01 R25: declared once). `--help` is rendered
//! from these doc comments, so they are user-facing text: placeholders like
//! `<id>`, never real ids or tracker numbers (STD-01 R23). The root page
//! lists those nouns under headings (`help`).

use crate::commands::Command;
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, FromArgMatches, Parser};
use cw_cli::Format;
use std::ffi::OsString;
use std::path::PathBuf;

/// Help heading for the flags every command accepts.
const GLOBAL: &str = "Global options";

const ROOT_EXAMPLES: &str = "\
Examples:
  scribe entry list --tag preferences --status active
  scribe entry show <id> --json
  scribe session list --project scribe
  scribe proposal list
  scribe consistency scan --json
  scribe tag list
  scribe search \"merge policy\" --kind session --since 2026-11-01
  scribe entry list | cut -f1,6

Exit status: 0 on success, 1 when a command fails, 2 on a usage error.";

/// Read the user's recorded preferences, and the records of the sessions they cataloged.
#[derive(Debug, Parser)]
#[command(
    name = "scribe",
    version,
    arg_required_else_help = true,
    propagate_version = true,
    after_help = ROOT_EXAMPLES
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) global: GlobalArgs,

    #[command(subcommand)]
    pub(crate) command: Command,
}

/// Flags accepted before or after any subcommand (STD-01 R4).
#[derive(Debug, Args)]
pub(crate) struct GlobalArgs {
    /// Transcript provider for hooks, discovery and catalog; picks [models] or [codex.models] for model calls [default: claude-code; allowed: claude-code, codex]
    #[arg(long, global = true, value_name = "PROVIDER", help_heading = GLOBAL)]
    pub(crate) provider: Option<scribe_core::Provider>,
    /// Data directory [default: $SCRIBE_ROOT, else ~/.scribe]
    #[arg(long, global = true, value_name = "DIR", help_heading = GLOBAL)]
    pub(crate) root: Option<PathBuf>,

    /// Output format; auto is a table on a terminal, tab-separated lines otherwise, and json is one JSON document [default: $SCRIBE_FORMAT, else auto]
    #[arg(long, global = true, value_enum, value_name = "FORMAT", help_heading = GLOBAL)]
    pub(crate) format: Option<Format>,

    /// Shorthand for --format json
    #[arg(long, global = true, help_heading = GLOBAL)]
    pub(crate) json: bool,
}

impl Cli {
    /// Parse a command line, including the checks clap cannot express.
    ///
    /// `conflicts_with` does not see a global flag given at a different
    /// subcommand level (`--json entry list --format table`), so the
    /// `--json`/`--format` conflict is checked here and reported as the same
    /// usage error (exit 2) clap would have produced.
    pub(crate) fn parse_args(args: &[OsString]) -> Result<Self, clap::Error> {
        let cmd = crate::help::install_root_help(Self::command(), crate::output::help_width());
        let matches = cmd.try_get_matches_from(args)?;
        let cli = Self::from_arg_matches(&matches)?;
        if cli.global.json && cli.global.format.is_some_and(|f| f != Format::Json) {
            return Err(Self::command().error(
                ErrorKind::ArgumentConflict,
                "--json cannot be used with a --format other than json",
            ));
        }
        if matches!(
            &cli.command,
            Command::Group(crate::commands::GroupCommand::Close {
                summary: None,
                no_summary: false,
                ..
            })
        ) {
            return Err(Self::command().error(
                ErrorKind::MissingRequiredArgument,
                "group close requires --summary or --no-summary; draft with `scribe group evidence <id> --json` and --summary, or run `orbit job run scribe-group-close`",
            ));
        }
        if matches!(cli.command, Command::Init(_)) && cli.global.provider.is_none() {
            return Err(Self::command().error(
                ErrorKind::MissingRequiredArgument,
                "scribe init requires --provider codex or --provider claude-code",
            ));
        }
        Ok(cli)
    }
}

impl GlobalArgs {
    /// The output format the flags ask for, or `None` to defer to the
    /// environment and then `auto`.
    pub(crate) fn requested_format(&self) -> Option<Format> {
        if self.json {
            Some(Format::Json)
        } else {
            self.format
        }
    }
}
