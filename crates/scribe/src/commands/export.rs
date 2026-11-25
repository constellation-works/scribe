//! `scribe export`: write every record kind to a directory the user
//! names, as Markdown and JSON in the shape of the old record files or as
//! JSONL, one line per row and one file per table
//! (docs/design/scribe/2_design.md §15.7).
//! It is the backup, the review copy and the way out of scribe. It reads
//! the store only: no lock, no store write, no audit line (STD-01 R31).

use crate::error::CliError;
use crate::output::Output;
use clap::{Args, ValueEnum};
use scribe_core::ExportFormat;
use scribe_core::apps::admin::{Command, exec};
use scribe_core::composition::Context;
use std::path::PathBuf;

pub(super) const EXAMPLES: &str = "\
Examples:
  scribe export ~/scribe-export
  scribe export --as jsonl ~/scribe-export-jsonl
  scribe export ~/scribe-export --json | jq '.files | length'";

/// The export's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ExportAs {
    /// The old store's files: sessions/, entries/, control.json, proposals.json, tags.json, marks.json, pickups.json, audit.jsonl
    Markdown,
    /// One line per row, <table>.jsonl per table
    Jsonl,
}

/// Arguments to `export`.
#[derive(Debug, Args)]
pub(crate) struct ExportArgs {
    /// Directory to write into: created owner-only, or refused unless empty
    #[arg(value_name = "DIR")]
    pub(crate) dir: PathBuf,

    /// markdown or jsonl [default: markdown]
    #[arg(long = "as", value_enum, value_name = "FORMAT", default_value_t = ExportAs::Markdown, hide_default_value = true)]
    pub(crate) export_as: ExportAs,
}

/// Run `export`.
pub(super) fn run(args: &ExportArgs, ctx: &Context) -> Result<Output, CliError> {
    let format = match args.export_as {
        ExportAs::Markdown => ExportFormat::Markdown,
        ExportAs::Jsonl => ExportFormat::Jsonl,
    };
    let command = Command::Export {
        dir: &args.dir,
        format,
    };
    Ok(super::admin::output(exec(ctx, command)?))
}
