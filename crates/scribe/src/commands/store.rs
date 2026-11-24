//! `scribe store migrate`: import the archives a file store left into
//! `scribe.db` now (docs/design/scribe/2_design.md §15.7, §15.9 phase 8).
//! Every write makes the same import on its own; this verb lets the user
//! choose the moment and is refused while a session is switched on, while
//! a rebuild checkpoint waits, or when there is nothing to import.

use crate::error::CliError;
use crate::output::Output;
use clap::{Args, Subcommand};
use scribe_core::apps::admin::{Command, exec};
use scribe_core::composition::Context;
use scribe_core::{Audit, AuditMeta};

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

impl StoreCommand {
    /// `store migrate` writes the store and calls no model, so it runs as
    /// one transaction with its audit line.
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::Migrate(_) => Some(AuditMeta {
                command: "store migrate",
                stepwise: false,
            }),
        }
    }
}

/// Run one `store` verb.
pub(super) fn run(
    command: StoreCommand,
    audit: Option<Audit>,
    ctx: &Context,
) -> Result<Output, CliError> {
    match command {
        StoreCommand::Migrate(MigrateArgs {}) => {
            let command = Command::Migrate;
            Ok(super::admin::output(exec(ctx, audit, command)?))
        }
    }
}
