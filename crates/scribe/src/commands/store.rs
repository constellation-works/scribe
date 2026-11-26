//! `scribe store migrate`: import the archives a file store left into
//! `scribe.db` now (docs/design/scribe/2_design.md §15.7, §15.9 phase 8).
//! Every write makes the same import on its own; this verb lets the user
//! choose the moment and is refused while a session is switched on, while
//! a rebuild checkpoint waits, or when there is nothing to import.
//! The `store` app's outcomes as output, `export`'s included.

use crate::error::CliError;
use crate::output::{ExportPayload, MigratePayload, Output, Record};
use clap::{Args, Subcommand};
use scribe_core::ExportFormat;
use scribe_core::apps::store::{Command, Outcome, exec};
use scribe_core::composition::Context;

pub(super) const EXAMPLES: &str = "\
Examples:
  scribe store migrate
  scribe store migrate --json";

const MIGRATE_EXAMPLES: &str = "\
Examples:
  scribe store migrate
  scribe store migrate --json | jq .archived";

/// `store` verbs.
#[derive(Debug, Subcommand)]
pub(crate) enum StoreCommand {
    /// Import the file store's archives into scribe.db now, then remove them
    ///
    /// Imports every pre-sqlite-<date>/ and pre-issue-<date>/ directory in the data directory, and any file an interrupted move left, into scribe.db's archive tables in one transaction that checks the record count of each kind, then removes the files. Every write makes this import on its own. Refused while a session is switched on, while a rebuild checkpoint waits, and when there is nothing to import. A store whose records never moved into scribe.db is refused: migrate it with an earlier scribe build first.
    #[command(after_help = MIGRATE_EXAMPLES)]
    Migrate(MigrateArgs),
}

/// Arguments to `store migrate`: none.
#[derive(Debug, Args)]
pub(crate) struct MigrateArgs {}

/// Run one `store` verb.
pub(super) fn run(command: StoreCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        StoreCommand::Migrate(MigrateArgs {}) => {
            let command = Command::Migrate;
            Ok(output(exec(ctx, command)?))
        }
    }
}

/// A `store` outcome as its output: `store migrate`'s records written, and
/// `export`'s directory and counts.
pub(in crate::commands) fn output(outcome: Outcome) -> Output {
    match outcome {
        Outcome::Migrated(moved) => {
            let moved = *moved;
            Output::written(
                Record::StoreMigrate(Box::new(MigratePayload {
                    sessions: moved.sessions,
                    switches: moved.switches,
                    cursors: moved.cursors,
                    moved_to: moved.moved_to,
                    entries: moved.entries,
                    claims: moved.claims,
                    tags: moved.tags,
                    marks: moved.marks,
                    pickups: moved.pickups,
                    audit_lines: moved.audit_lines,
                    audit_torn: moved.audit_torn,
                    archives: moved.archives,
                    archived: moved.archived,
                })),
                true,
            )
        }
        Outcome::Exported(written) => {
            let written = *written;
            let exported = written.exported;
            Output::Show {
                record: Record::Export(Box::new(ExportPayload {
                    dir: written.dir.display().to_string(),
                    format: match written.format {
                        ExportFormat::Markdown => "markdown",
                        ExportFormat::Jsonl => "jsonl",
                    },
                    sessions: exported.sessions,
                    entries: exported.entries,
                    claims: exported.claims,
                    audit_lines: exported.audit_lines,
                    files: exported.files,
                    skipped: exported
                        .skipped
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect(),
                })),
            }
        }
    }
}
