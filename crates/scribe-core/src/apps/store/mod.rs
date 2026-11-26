//! The `store` app: the upkeep of the one persisted resource and its way out,
//! `scribe store migrate` and `scribe export` (2_design.md §15). Both act on
//! the store (`scribe.db` and the archives beside it), so they share one
//! application unit and one entry point (STD-02@8 §R45; ORB-28770). Migrate
//! imports a file store's archives into the store; export writes every record
//! kind out of it. Neither reads another app's rules.
//!
//! Each command is one call to the store. `store migrate` refuses through the
//! app's rules before the store imports anything.

mod migrate;
pub mod rules;

#[cfg(any(test, feature = "test-support"))]
pub use migrate::MigrateTestWrites;

use crate::composition::Context;
use crate::runtime::audit::{Audit, AuditMeta, AuditSubject, Audited, Line, guarded};
use crate::runtime::errors::CommandError;
use crate::{ExportCounts, ExportFormat, Imported};
use std::path::{Path, PathBuf};

/// A `store` command: `store migrate`, or `export`.
pub enum Command<'i> {
    /// `store migrate`: import a file store's archives, dated when it runs.
    Migrate,
    /// `export`: write every record kind into `dir` as `format`.
    Export {
        /// The directory to write into.
        dir: &'i Path,
        /// Markdown or JSONL.
        format: ExportFormat,
    },
}

impl Command<'_> {
    /// What the audit log records for this command, or `None` when it is
    /// not audited: a read writes nothing, not even an audit line (STD-01
    /// R31). Exhaustive, so a new command cannot compile without deciding
    /// (STD-02 R27).
    #[must_use]
    pub fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::Migrate => Some(AuditMeta {
                command: "store migrate",
                stepwise: false,
            }),
            // An export writes nothing to the store (STD-01 R31).
            Self::Export { .. } => None,
        }
    }

    /// The record and agent this command names: none, since the store
    /// commands name theirs in their audit lines.
    #[must_use]
    pub fn subject(&self, _ctx: &Context) -> AuditSubject {
        AuditSubject::default()
    }
}

/// What a `store` command returns.
pub enum Outcome {
    /// What `store migrate` imported.
    Migrated(Box<Imported>),
    /// What `export` wrote.
    Exported(Box<ExportWritten>),
}

/// What `export` wrote, and where and how.
pub struct ExportWritten {
    /// The counts and files written.
    pub exported: ExportCounts,
    /// The directory written into.
    pub dir: PathBuf,
    /// The format written.
    pub format: ExportFormat,
}

/// Why a `store` command did not run.
pub type Error = crate::Error;

/// Run `command` inside the audit guard when [`Command::audit`] declares
/// one. The store is below the binary and calls no other app, so it has no
/// `run` of its own.
///
/// # Errors
///
/// `migrate_blocked` and the store's read and write errors for `migrate`;
/// the store's export errors for `export`.
pub fn exec(ctx: &Context, command: Command<'_>) -> Result<Outcome, CommandError> {
    let audit = Audit::declared(command.audit(), || command.subject(ctx));
    guarded(ctx, audit, || {
        Ok(match command {
            Command::Migrate => {
                Outcome::Migrated(Box::new(migrate::migrate(ctx.store(), ctx.now())?))
            }
            Command::Export { dir, format } => Outcome::Exported(Box::new(ExportWritten {
                exported: ctx.store().export(dir, format)?,
                dir: dir.to_path_buf(),
                format,
            })),
        })
    })
}

/// `store migrate` writes one line, naming the records. `export` writes
/// nothing to the store: never audited.
impl Audited for Outcome {
    fn lines(&self) -> Vec<Line> {
        match self {
            Self::Migrated(_) => vec![Line::success(Some(String::from("store:records")))],
            Self::Exported(_) => vec![Line::success(None)],
        }
    }
}
