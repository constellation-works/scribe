//! `scribe store migrate`: move a file store's sessions into `scribe.db`
//! now (docs/design/scribe/2_design.md §15.7, §15.11). Every write makes
//! the same move on its own; this verb lets the user choose the moment and
//! is refused while a session is switched on, while a rebuild checkpoint
//! waits, or once the sessions moved.

use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{MigratePayload, Output, Record};
use clap::{Args, Subcommand};

pub(super) const EXAMPLES: &str = "\
Examples:
  scribe store migrate
  scribe store migrate --json";

const MIGRATE_EXAMPLES: &str = "\
Examples:
  scribe store migrate
  scribe store migrate --json | jq .moved_to";

/// `store` verbs.
#[derive(Debug, Subcommand)]
pub(crate) enum StoreCommand {
    /// Move session files into scribe.db now, keeping the old files aside
    ///
    /// Imports sessions/ and control.json's switches and Codex cursors into scribe.db in one transaction that checks the record count, then moves the files to pre-sqlite-<date>/ in the data directory, which scribe never deletes. Every write makes this move on its own. Refused while a session is switched on, while a rebuild checkpoint waits, and once the sessions moved.
    #[command(after_help = MIGRATE_EXAMPLES)]
    Migrate(MigrateArgs),
}

/// Arguments to `store migrate`: none.
#[derive(Debug, Args)]
pub(crate) struct MigrateArgs {}

impl StoreCommand {
    /// `store migrate` writes the store, so it is audited.
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::Migrate(_) => Some(AuditMeta {
                command: "store migrate",
            }),
        }
    }
}

/// Run one `store` verb.
pub(super) fn run(command: StoreCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        StoreCommand::Migrate(MigrateArgs {}) => {
            let moved = ctx.store().migrate_sessions(ctx.now())?;
            Ok(Output::written(
                Record::StoreMigrate(Box::new(MigratePayload {
                    sessions: moved.sessions,
                    switches: moved.switches,
                    cursors: moved.cursors,
                    moved_to: moved.moved_to,
                })),
                true,
            ))
        }
    }
}
